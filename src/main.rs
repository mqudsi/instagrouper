#![allow(special_module_name)]

mod lib;

use std::fmt::Display;
use std::path::PathBuf;

macro_rules! exit {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
        std::process::exit(1);
    }};
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut paths = Vec::new();
    let mut out_dir = None;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--outdir") => {
                let temp = args.next().or_exit("Missing --outdir value!");
                let path = PathBuf::from(temp);
                if !path.exists() {
                    exit!("outdir not found!");
                }
                out_dir = Some(path);
            }
            Some(opt) if opt.starts_with("--") => exit!("Unrecognized option {opt}"),
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    lib::group(&paths).unwrap();
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
