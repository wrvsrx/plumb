use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::server::{serve, ServeConfig};

#[derive(Debug, Parser)]
#[command(name = "plumb site", about = "Serve a plumb workspace Web app")]
struct SiteConfig {
    #[command(subcommand)]
    command: SiteCommand,
}

#[derive(Debug, Subcommand)]
enum SiteCommand {
    /// Serve the workspace as a dynamic local Web app.
    Serve(ServeConfig),
}

pub fn run_site_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let config = match SiteConfig::try_parse_from(args) {
        Ok(config) => config,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match config.command {
        SiteCommand::Serve(config) => serve(config),
    }
}
