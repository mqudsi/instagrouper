#![allow(special_module_name)]

mod lib;

use jiff::Timestamp;
use serde::Serialize;
use size::Size;
use std::fmt::Display;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

macro_rules! exit {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
        std::process::exit(1);
    }};
}

fn print_usage() {
    eprintln!("instagroup [--out-dir OUTDIR] path1.mp4 path2.mp4 ...");
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut paths = Vec::new();
    let mut out_dir = PathBuf::from("./");

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-o" | "--outdir" | "--out-dir") => {
                let temp = args.next().or_exit("Missing --out-dir value!");
                let path = PathBuf::from(temp);
                if !path.exists() {
                    exit!("outdir not found!");
                }
                out_dir = path;
            }
            Some("-h" | "--help") => {
                print_usage();
                std::process::exit(0);
            }
            Some(opt) if opt.starts_with("-") => exit!("Unrecognized option {opt}"),
            _ => {
                if let Some(ext) = arg.as_bytes().last_chunk::<4>()
                    && (true || &ext.to_ascii_lowercase() == b".mp4")
                {
                    let path = PathBuf::from(arg);
                    if !path.exists() {
                        exit!("{}: Path not found", path.display());
                    }
                    paths.push(path);
                }
            }
        }
    }

    if paths.is_empty() {
        print_usage();
        exit!("");
    }

    // Group input files into groups matching a single original attachment
    let groups = lib::group(&paths).unwrap();

    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let chunk_size = groups.len().div_ceil(num_cpus);

    let results: Vec<Attachment> = std::thread::scope(|s| {
        groups
            .chunks(chunk_size)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let out_dir = out_dir.clone();
                s.spawn(move || {
                    let mut local_results = Vec::new();
                    for (in_chunk_idx, group) in chunk.iter().enumerate() {
                        assert!(!group.is_empty());
                        let n = chunk_idx * chunk_size + in_chunk_idx;

                        let timestamp = group.iter().map(|mi| mi.timestamp).min().unwrap();
                        let sources = group.iter().map(|mi| mi.path.clone()).collect();
                        let name0 = group[0].path.file_name().unwrap().to_string_lossy();

                        if group.len() == 1 && group[0].is_image() {
                            local_results.push(Attachment {
                                name: name0.to_string(),
                                size: group[0].size,
                                size_pretty: Size::from_bytes(group[0].size).to_string(),
                                timestamp,
                                path: std::fs::canonicalize(&group[0].path).unwrap(),
                                duration: Duration::ZERO.into(),
                                kind: "image",
                                sources,
                                thumbnail: std::fs::canonicalize(&group[0].path).unwrap(),
                            });
                            continue;
                        }

                        // Try to use up to second _ as a prefix, new uuid otherwise.
                        let uuid;
                        let stub =
                            if let Some(idx) = name0.match_indices('_').nth(1).map(|(i, _)| i) {
                                &name0[..idx]
                            } else {
                                uuid = Uuid::now_v7().to_string();
                                &uuid
                            };

                        let mp4name = format!("{stub}_{n:0>3}.mp4");
                        let mp4path = out_dir.join(&mp4name);
                        let kind = lib::merge(group, &mp4path).unwrap();

                        let jpgname = format!("{stub}_{n:0>3}.jpg");
                        let jpgpath = out_dir.join(jpgname);
                        lib::thumbnail(&mp4path, &jpgpath).unwrap();

                        let size = mp4path.metadata().unwrap().len();
                        local_results.push(Attachment {
                            name: mp4name,
                            path: std::fs::canonicalize(mp4path).unwrap(),
                            timestamp,
                            size,
                            size_pretty: Size::from_bytes(size).to_string(),
                            kind,
                            thumbnail: std::fs::canonicalize(&jpgpath).unwrap(),
                            duration: group[0].duration.into(),
                            sources,
                        });
                    }
                    local_results
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });

    eprintln!(
        "Merged {} files into {} attachments",
        paths.len(),
        groups.len()
    );

    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}

#[derive(Serialize)]
struct Attachment {
    pub name: String,
    pub path: PathBuf,
    pub timestamp: Timestamp,
    pub size: u64,
    pub size_pretty: String,
    pub kind: &'static str,
    pub thumbnail: PathBuf,
    pub duration: lib::PrettyDuration,
    pub sources: Vec<PathBuf>,
}

trait OrExit {
    type T: Sized;
    fn or_exit(self, msg: &str) -> Self::T;
}

impl<T: Sized, E: Display> OrExit for Result<T, E> {
    type T = T;

    fn or_exit(self, msg: &str) -> Self::T {
        self.unwrap_or_else(|err| exit!("{msg}: {err}"))
    }
}

impl<T: Sized> OrExit for Option<T> {
    type T = T;

    fn or_exit(self, msg: &str) -> Self::T {
        self.unwrap_or_else(|| exit!("{msg}"))
    }
}
