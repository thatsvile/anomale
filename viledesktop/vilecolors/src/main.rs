use std::path::PathBuf;
use std::process::ExitCode;

use vile_protocol::color_cache_dir;
use vilecolors::{apply, parse_cmd, run_lab, ColorsBinCmd};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    match parse_cmd(std::env::args().skip(1))? {
        ColorsBinCmd::Apply { recipe, image } => {
            let palette = apply(&recipe, &PathBuf::from(image))?;
            eprintln!(
                "wrote {} colors to {}",
                palette.colors.len(),
                color_cache_dir().display()
            );
            Ok(())
        }
        ColorsBinCmd::Lab(opts) => {
            let out = opts.out.clone();
            if let Some(pick) = run_lab(opts)? {
                let json = serde_json::to_string(&pick)?;
                if let Some(path) = out {
                    std::fs::write(&path, format!("{json}\n"))?;
                } else {
                    println!("{json}");
                }
            }
            Ok(())
        }
    }
}
