#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::Path;
use std::process::ExitCode;

fn render_images(folder: Option<String>) -> ExitCode {
    let Some(folder) = folder else {
        eprintln!("render-images needs the scan folder as its argument");
        return ExitCode::FAILURE;
    };
    match lost_scores_gui::share::render::render_folder(Path::new(&folder)) {
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
        return render_images(args.next());
    }
    match lost_scores_gui::app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:?}");
            ExitCode::FAILURE
        }
    }
}
