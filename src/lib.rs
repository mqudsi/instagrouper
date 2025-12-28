use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize, Serializer};
use std::fmt::{self, Display};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

macro_rules! defer {
    ($($body:tt)*) => {
        let _guard = {
            struct D<F: FnMut()>(F);
            impl<F: FnMut()> Drop for D<F> {
                fn drop(&mut self) { (self.0)(); }
            }
            D(|| { $($body)* })
        };
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    Audio,
    Video,
    Image,
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
            // If audio/image, group must not already have audio/image.
            // If video, group must not already have this resolution.
            let already_has_resolution = group.iter().any(|other| match mi.media {
                MediaType::Audio => other.is_audio(),
                MediaType::Image => other.is_image(),
                MediaType::Video => other.resolution == mi.resolution,
            });

            if already_has_resolution {
                continue;
            }

            // Hack to skip thumbnails
            if mi.is_image() {
                if best_match.is_none() {
                    if group.iter().any(|other| other.resolution == mi.resolution) {
                        best_match = Some((idx, Default::default()));
                    }
                }
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

    let max_divergence = groups
        .iter()
        .map(|g| {
            let mut candidates = g.iter().filter(|mi| !mi.is_image());
            // Members are inserted by decreasing duration
            let first = candidates.next();
            let last = candidates.last();
            if let (Some(first), Some(last)) = (first, last) {
                first.duration - last.duration
            } else {
                // Less than two non-image files in group
                Duration::ZERO
            }
        })
        .max();

    if let Some(max) = max_divergence {
        eprintln!("max duration divergence: {max:?}");
    }

    Ok(groups)
}

pub fn merge(group: &[MediaInfo], out: &Path) -> Result<&'static str> {
    assert!(!group.is_empty());

    let audio = group.iter().filter(|mi| mi.is_audio()).next();
    let video = group
        .iter()
        .filter(|mi| mi.is_video())
        .max_by_key(|mi| mi.resolution);

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

    let play_overlay = {
        let mut path = std::env::temp_dir();
        path.push(Uuid::now_v7().to_string());
        path.set_extension("png");
        path
    };

    File::create(&play_overlay)
        .and_then(|mut f| f.write_all(play_overlay_webp()))
        .with_context(|| format!("Error writing play overlay to {}", play_overlay.display()))?;

    defer! {
        eprintln!("Deleting {}", play_overlay.display());
        if let Err(err) = std::fs::remove_file(&play_overlay) {
            eprintln!("Error cleaning up play overlay icon at {}: {err}", play_overlay.display());
        }
    }

    let start = match mi.duration.as_secs() {
        ..1 => "0",
        ..6 => "2.0",
        _ => "5.0",
    };

    let ffmpeg = Command::new("ffmpeg")
        .arg("-hide_banner")
        // .arg("-v")
        // .arg("error")
        .arg("-ss")
        .arg(start)
        .arg("-i")
        .arg(&src)
        // Loop the image so it's always available at the same timestamp as the video
        .arg("-loop")
        .arg("1")
        .arg("-i")
        .arg(&play_overlay)
        .arg("-filter_complex")
        // // FFmpeg 7+ Logic:
        // // [0:v]split[main][ref] -> Create two copies of the video.
        // // [1:v][ref]scale=...[logo] -> Scale the logo (input 1) using the 'ref' copy for dimensions.
        // //   'rw' and 'rh' are the Reference Width/Height of the 2nd input ([ref]).
        // // [main][logo]overlay=... -> Overlay the scaled logo onto the 'main' video copy.
        // .arg(
        //     "[0:v]split[main][ref]; \
        //   [1:v][ref]scale=w='min(rw,rh)*0.4':h=-1[logo]; \
        //   [main][logo]overlay=(W-w)/2:(H-h)/2:shortest=1",
        // )
        // FFmpeg 6.0 and below logic (works on 7.0 but gives a deprecation warning):
        // [1:v][0:v] takes (1) logo and (2) video.
        // 'main_w' and 'main_h' refer to the second input ([0:v] / the video).
        // It outputs two streams: [logo] (the scaled icon) and [video] (the original video).
        // We then overlay [logo] on [video].
        .arg(
            "[1:v][0:v]scale2ref=w='min(main_w,main_h)*0.4':h='min(main_w,main_h)*0.4'[logo][video]; \
              [video][logo]overlay=(W-w)/2:(H-h)/2:shortest=1",
        )
        .arg("-frames:v")
        .arg("1")
        .arg("-c:v")
        .arg("mjpeg")
        // .arg("-q:v")
        // .arg("2")
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

    if !out.exists() {
        std::io::stderr().lock().write_all(&ffmpeg.stderr).unwrap();
        std::io::stdout().lock().write_all(&ffmpeg.stdout).unwrap();
        bail!("Failed to generate screenshot with ffmpeg, refer to output.");
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
    pub size: u64,
    pub duration: Duration,
    pub timestamp: Timestamp,
    pub resolution: Option<Resolution>,
    pub bit_rate: Option<u32>,
}

impl MediaInfo {
    pub fn is_audio(&self) -> bool {
        self.media == MediaType::Audio
    }

    pub fn is_video(&self) -> bool {
        self.media == MediaType::Video
    }

    pub fn is_image(&self) -> bool {
        self.media == MediaType::Image
    }
}

pub fn deserialize_duration<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: From<Duration>,
{
    let secs = String::deserialize(deserializer)?
        .parse::<f64>()
        .context("Error deserializing Duration from string")
        .map_err(serde::de::Error::custom)?;

    Ok(T::from(Duration::from_secs_f64(secs)))
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
        /// Defaults to [`Duration::ZERO`] if field isn't present
        #[serde(default, deserialize_with = "deserialize_duration")]
        pub duration: Duration,
        pub bit_rate: Option<String>,
        pub tags: Option<Tags>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Tags {
        pub creation_time: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Stream {
        pub codec_type: String,
        pub codec_name: String,
        pub width: Option<u16>,
        pub height: Option<u16>,
        pub bit_rate: Option<String>,
        /// Defaults to `None` if field isn't present
        #[serde(default, deserialize_with = "deserialize_duration")]
        pub duration: Option<Duration>,
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

    if ffprobe.streams.is_empty() {
        bail!("Empty media file provided (no streams)");
    }

    let mut media_info = MediaInfo {
        path: path.to_owned(),
        stream_count: ffprobe.format.nb_streams,
        size: ffprobe.format.size.parse().expect("Failed to parse size"),
        media: match ffprobe.streams[0].codec_type.as_str() {
            "audio" => MediaType::Audio,
            "video" if matches!(ffprobe.streams[0].codec_name.as_str(), "png" | "mjpeg") => {
                MediaType::Image
            }
            "video" => MediaType::Video,
            other => panic!("Unexpected media type {other}"),
        },
        codec: ffprobe.streams[0].codec_name.clone(),
        duration: ffprobe.streams[0]
            .duration
            .unwrap_or(ffprobe.format.duration),
        bit_rate: ffprobe.streams[0]
            .bit_rate
            .as_ref()
            .or_else(|| ffprobe.format.bit_rate.as_ref())
            .map(|s| s.parse().expect("Failed to parse bitrate")),
        timestamp: match ffprobe.format.tags.and_then(|t| t.creation_time) {
            Some(ctime) => parser
                .parse_timestamp(&ctime)
                .expect("Failed to parse creation time"),
            None => path
                .metadata()
                .and_then(|md| md.created())
                .unwrap_or_else(|_| SystemTime::now())
                .try_into()
                .expect("Failed to convert SystemTime to Timestamp!"),
        },
        resolution: None,
    };
    if matches!(media_info.media, MediaType::Video | MediaType::Image) {
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

fn play_overlay_webp() -> &'static [u8] {
    include_bytes!("../media/play-overlay.webp")
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
