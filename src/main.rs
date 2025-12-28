#![allow(special_module_name)]

mod lib;

use serde::Serialize;
use size::Size;
use std::ffi::OsString;
use std::fmt::Display;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
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
                let temp = args.next().or_exit("Missing --outdir value!");
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
                if let Some(ext) = arg.as_bytes().last_chunk::<4>() {
                    if &ext.to_ascii_lowercase() == b".mp4" {
                        let path = PathBuf::from(arg);
                        if !path.exists() {
                            exit!("{}: Path not found", path.display());
                        }
                        paths.push(path);
                    }
                }
            }
        }
    }

    if paths.is_empty() {
        print_usage();
        exit!("");
    }

    let groups = lib::group(&paths).unwrap();
    let mut results = Vec::with_capacity(groups.len());
    for (n, group) in groups.iter().enumerate() {
        let fname = group[0].path.file_name().unwrap().to_string_lossy();

        // Take up to second _ in filename as prefix, if possible
        let uuid;
        let stub = if let Some(idx) = fname.match_indices('_').nth(1).map(|(i, _)| i) {
            &fname[..idx]
        } else {
            uuid = Uuid::now_v7().to_string();
            &uuid
        };

        let mp4name = format!("{stub}_{n:0>3}.mp4");
        let mp4path = out_dir.join(mp4name);
        let kind = lib::merge(&group, Path::new(&mp4path)).unwrap();

        let jpgname = format!("{stub}_{n:0>3}.jpg");
        let jpgpath = out_dir.join(jpgname);
        lib::thumbnail(&mp4path, &jpgpath).unwrap();

        let size = mp4path.metadata().unwrap().len();
        results.push(Attachment {
            name: mp4path.file_name().unwrap().to_string_lossy().to_string(),
            size,
            pretty_size: Size::from_bytes(size).to_string(),
            kind,
            path: std::fs::canonicalize(mp4path).unwrap(),
            thumbnail: jpgpath,
            duration: group[0].duration.into(),
            sources: group.iter().map(|mi| mi.path.clone()).collect(),
        })
    }

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
    pub size: u64,
    pub pretty_size: String,
    pub kind: &'static str,
    pub path: PathBuf,
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
