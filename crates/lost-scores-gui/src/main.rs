#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::OsString;
use std::process::ExitCode;

use lost_scores_gui::share::render::{FAILURE_PREFIX, RenderArgs, guarded, render_folder};

fn render_images(args: impl Iterator<Item = OsString>) -> ExitCode {
    let args = match RenderArgs::parse(args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match guarded(|| render_folder(&args)) {
        Ok(output) => {
            for warning in output.warnings {
                eprintln!("{warning}");
            }
            for path in output.paths {
                println!("{}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{FAILURE_PREFIX}{error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|first| first == "render-images") {
        return render_images(args);
    }
    match lost_scores_gui::app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:?}");
            ExitCode::FAILURE
        }
    }
}
