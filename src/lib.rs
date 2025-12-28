use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt::{self, Display};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use jiff::Timestamp;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    Audio,
    Video,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Ord, Copy)]
pub struct Resolution {
    pub width: u16,
    pub height: u16,
}

impl PartialOrd for Resolution {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (self.width as usize * self.height as usize)
            .partial_cmp(&(other.width as usize * other.height as usize))
    }
}

#[test]
fn resolution_cmp() {
    assert!(
        Resolution {
            width: 720,
            height: 400
        }
        .cmp(&Resolution {
            width: 120,
            height: 200
        })
        .is_gt()
    );
}

#[test]
fn resolution_opt_cmp() {
    assert!(
        Some(Resolution {
            width: 720,
            height: 400
        })
        .cmp(&None)
        .is_gt()
    );
}

impl Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{width}x{height}",
            width = self.width,
            height = self.height
        )
    }
}

/// Group paths into files belonging to the same attachment
pub fn group<P: AsRef<Path>>(paths: &[P]) -> Result<Vec<Vec<MediaInfo>>> {
    let mut media_info = Vec::with_capacity(paths.len());

    for path in paths {
        let path = path.as_ref();
        let mi = identify(path).with_context(|| format!("Error identifying {}", path.display()))?;
        media_info.push(mi);
    }

    // Sort by duration to ensure we process similar files together first
    media_info.sort_by_key(|mi| std::cmp::Reverse(mi.duration));

    let mut groups: Vec<Vec<MediaInfo>> = Vec::new();

    // Max deviation allowed between two encodes of the same original media
    let max_delta = Duration::from_secs_f64(0.7);

    for mi in media_info {
        let mut best_match: Option<(usize, Duration)> = None; // (idx, Duration)

        for (idx, group) in groups.iter().enumerate() {
            // If audio, group must not already have audio.
            // If video, group must not already have this resolution.
            let already_has_resolution = group.iter().any(|other| {
                if mi.media == MediaType::Audio {
                    other.media == MediaType::Audio
                } else {
                    other.resolution == mi.resolution && other.media == MediaType::Video
                }
            });

            if already_has_resolution {
                continue;
            }

            let delta = mi.duration.abs_diff(group[0].duration);
            if delta <= max_delta {
                // Take closest matching duration
                match best_match {
                    None => best_match = Some((idx, delta)),
                    Some((_, best_delta)) => {
                        if delta < best_delta {
                            best_match = Some((idx, delta));
                        }
                    }
                }
            }
        }

        if let Some((idx, _)) = best_match {
            groups[idx].push(mi);
        } else {
            // No compatible group found, create a new one
            groups.push(vec![mi]);
        }
    }

    eprintln!(
        "Organized {} media into {} groups",
        paths.len(),
        groups.len()
    );

    let max_divergence = groups
        .iter()
        .map(|g| g.first().unwrap().duration - g.last().unwrap().duration)
        .max()
        .unwrap();

    eprintln!("max duration divergence: {max_divergence:?}");

    Ok(groups)
}

pub fn merge(group: &[MediaInfo], out: &Path) -> Result<&'static str> {
    assert!(!group.is_empty());

    let audio = group.iter().filter(|mi| mi.is_audio()).next();
    let video = group.iter().max_by_key(|mi| mi.resolution);

    let (Some(audio), Some(video)) = (audio, video) else {
        // Missing either audio or video
        eprintln!("Copying source file as-is to {}", out.display());
        std::fs::copy(&group[0].path, out)
            .with_context(|| format!("Error writing to destination {}", out.display()))?;
        return Ok(if audio.is_some() { "audio" } else { "video" });
    };

    let ffmpeg = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(&audio.path)
        .arg("-i")
        .arg(&video.path)
        .arg("-c")
        .arg("copy")
        .arg("-f")
        .arg("mp4")
        .arg(out)
        .output()
        .context("Error running ffmpeg!")?;

    if !ffmpeg.status.success() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(&ffmpeg.stderr);
        bail!("Error merging media");
    }

    let fname = out.file_name().unwrap();
    eprintln!("Merged audio and video into {}", fname.display());

    Ok("audio+video")
}

pub fn thumbnail(src: &Path, out: &Path) -> Result<()> {
    let mi = identify(src).context("Error identifying file to screenshot")?;

    if mi.is_audio() && mi.stream_count == 1 {
        let mut file = File::create(out).with_context(|| {
            format!("Error creating screenshot output file at {}", out.display())
        })?;
        file.write_all(audio_only_png())
            .with_context(|| format!("Error writing screenshot to {}", out.display()))?;
        return Ok(());
    }

    let start = match mi.duration.as_secs() {
        ..1 => "0",
        ..6 => "2.0",
        _ => "5.0",
    };

    let ffmpeg = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(&src)
        .arg("-ss")
        .arg(start)
        .arg("-frames:v")
        .arg("1")
        .arg("-c:v")
        .arg("mjpeg")
        .arg("-f")
        .arg("image2")
        .arg(out)
        .output()
        .context("Error running ffmpeg!")?;

    if !ffmpeg.status.success() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(&ffmpeg.stderr);
        bail!("Error taking screenshot");
    }

    let fname = out.file_name().unwrap();
    eprintln!("Screenshot saved to {}", fname.display());

    Ok(())
}

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct MediaInfo {
    pub stream_count: u8,
    pub media: MediaType,
    pub path: PathBuf,
    pub codec: String,
    pub size: usize,
    pub duration: Duration,
    pub timestamp: Timestamp,
    pub resolution: Option<Resolution>,
    pub bit_rate: u32,
}

impl MediaInfo {
    pub fn is_audio(&self) -> bool {
        self.media == MediaType::Audio
    }

    pub fn is_video(&self) -> bool {
        self.media == MediaType::Video
    }
}

pub fn identify<'a>(path: &'a Path) -> Result<MediaInfo> {
    #[derive(Debug, Deserialize)]
    pub struct Ffprobe {
        pub format: Format,
        pub streams: Vec<Stream>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Format {
        pub size: String,
        pub nb_streams: u8,
        pub duration: String,
        pub bit_rate: String,
        pub tags: Option<Tags>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Tags {
        pub creation_time: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Stream {
        pub codec_type: String,
        #[serde(rename = "codec_name")]
        pub codec: String,
        pub width: Option<u16>,
        pub height: Option<u16>,
        pub bit_rate: Option<String>,
        pub duration: String,
    }

    let ffprobe = Command::new("ffprobe")
        .arg("-hide_banner")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_entries")
        .arg("stream")
        .arg("-v")
        .arg("error")
        .arg(path)
        .output()
        .context("Error running ffprobe!")?;

    if !ffprobe.status.success() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(&ffprobe.stderr);
        bail!("Error analyzing media");
    }

    let parser = jiff::fmt::temporal::DateTimeParser::new();
    let ffprobe: Ffprobe =
        serde_json::from_slice(&ffprobe.stdout).expect("Internal error decoding ffprobe output!");
    let mut media_info = MediaInfo {
        path: path.to_owned(),
        stream_count: ffprobe.format.nb_streams,
        size: ffprobe.format.size.parse().expect("Failed to parse size"),
        media: match ffprobe.streams[0].codec_type.as_str() {
            "audio" => MediaType::Audio,
            "video" => MediaType::Video,
            other => panic!("Unexpected media type {other}"),
        },
        codec: ffprobe.streams[0].codec.clone(),
        duration: Duration::from_secs_f64(ffprobe.streams[0].duration.parse().unwrap()),
        bit_rate: ffprobe.streams[0]
            .bit_rate
            .as_ref()
            .unwrap_or(&ffprobe.format.bit_rate)
            .parse()
            .expect("Failed to parse bitrate"),
        timestamp: match ffprobe.format.tags.and_then(|t| t.creation_time) {
            Some(ctime) => parser
                .parse_timestamp(&ctime)
                .expect("Failed to parse creation time"),
            None => path
                .metadata()
                .expect("Failed to load media metadata")
                .created()
                .unwrap()
                .try_into()
                .unwrap(),
        },
        resolution: None,
    };
    if matches!(media_info.media, MediaType::Video) {
        media_info.resolution = Resolution {
            width: ffprobe.streams[0].width.unwrap(),
            height: ffprobe.streams[0].height.unwrap(),
        }
        .into();
    }

    Ok(media_info)
}

fn audio_only_png() -> &'static [u8] {
    include_bytes!("../media/audio-only.png")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrettyDuration(pub Duration);

impl fmt::Display for PrettyDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_secs = self.0.as_secs();
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;
        let millis = self.0.subsec_millis();

        if f.alternate() {
            // hh:mm:ss.mmm
            write!(
                f,
                "{:02}:{:02}:{:02}.{:.3}",
                hours, minutes, seconds, millis
            )
        } else if hours > 0 {
            // hh:mm:ss
            write!(f, "{:02}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            // mm:ss
            write!(f, "{:02}:{:02}", minutes, seconds)
        }
    }
}

impl Serialize for PrettyDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{:#}", self);
        serializer.serialize_str(&s)
    }
}

impl From<Duration> for PrettyDuration {
    fn from(value: Duration) -> Self {
        Self(value)
    }
}
