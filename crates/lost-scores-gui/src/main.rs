#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

use lost_scores_gui::share::render::{RenderArgs, render_folder};

fn render_images(args: impl Iterator<Item = String>) -> ExitCode {
    let args = match RenderArgs::parse(args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match render_folder(&args) {
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
            eprintln!("cannot render the share images: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("render-images") {
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
