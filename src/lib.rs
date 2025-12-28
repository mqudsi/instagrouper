use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Display;
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

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct Resolution {
    pub width: u16,
    pub height: u16,
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

    // Each attachment can only have one audio version, so start with that
    let mut added = HashSet::with_capacity(media_info.len());
    let mut groups = Vec::with_capacity(media_info.len());
    for mi in media_info.iter().filter(|mi| mi.media == MediaType::Audio) {
        groups.push(vec![mi]);
        added.insert(mi);
    }

    while added.len() != media_info.len() {
        for mi in &media_info {
            if added.contains(mi) {
                continue;
            }
            let candidate_groups = groups
                .iter()
                .filter(|g| !g.iter().any(|other| other.resolution == mi.resolution));
            let best_group_idx = candidate_groups
                .enumerate()
                .min_by_key(|(_, g)| {
                    g.iter()
                        .map(|other| other.duration.abs_diff(mi.duration))
                        .min()
                        .unwrap()
                })
                .unwrap()
                .0;
            groups[best_group_idx].push(mi);
            added.insert(mi);
        }
    }

    eprintln!(
        "Organized {} media into {} groups",
        media_info.len(),
        groups.len()
    );

    dbg!(&groups);

    Ok(groups
        .into_iter()
        .map(|g| g.into_iter().cloned().collect())
        .collect())
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
