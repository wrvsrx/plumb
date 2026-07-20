use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "plumb-fmt", about = "Format plumb documents")]
struct Args {
    #[arg(long)]
    check: bool,
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("plumb-fmt: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<bool, String> {
    if args.paths.is_empty() {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        let formatted = format_source(&source, "stdin")?;
        if args.check {
            return Ok(source == formatted);
        }
        print!("{formatted}");
        return Ok(true);
    }

    let mut unchanged = true;
    for path in args.paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let formatted = format_source(&source, &path.display().to_string())?;
        if source == formatted {
            continue;
        }
        unchanged = false;
        if args.check {
            eprintln!("would reformat {}", path.display());
        } else {
            fs::write(&path, formatted)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
    }
    Ok(!args.check || unchanged)
}

fn format_source(source: &str, name: &str) -> Result<String, String> {
    plumb_format::format(source).map_err(|_| format!("{name} has syntax errors"))
}
