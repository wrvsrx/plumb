use clap::Parser;
use std::{ffi::OsString, process::ExitCode};

#[derive(Parser)]
#[command(name = "plumb lsp", about = "Run the plumb language server over stdio")]
struct Args {
    /// Override a workspace configuration field with a TOML value (repeatable).
    #[arg(long = "config", value_name = "KEY=VALUE")]
    overrides: Vec<String>,
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    if let Err(error) = plumb_workspace::WorkspaceConfig::parse("", &args.overrides) {
        eprintln!("plumb lsp: {error}");
        return ExitCode::from(2);
    }
    crate::run_lsp_with_config(args.overrides);
    ExitCode::SUCCESS
}
