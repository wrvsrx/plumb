use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let _executable = args.next();
    let Some(command) = args.next() else {
        print_help();
        return ExitCode::from(2);
    };
    let rest = args.collect::<Vec<_>>();

    match command.to_str() {
        Some("fmt") => plumb_format::run_cli(delegated_args("plumb fmt", rest)),
        Some("export") => {
            if wants_help(&rest) {
                println!("Usage: plumb export [PATH]\n\nEmit a Pandoc JSON document. Reads stdin when PATH is omitted.");
                ExitCode::SUCCESS
            } else {
                plumb_export::run_cli(delegated_args("plumb export", rest))
            }
        }
        Some("note" | "task") => {
            let mut delegated = vec![OsString::from("plumb"), command];
            delegated.extend(rest);
            plumb_notes::run_cli(delegated)
        }
        Some("lsp") => {
            if wants_help(&rest) {
                println!("Usage: plumb lsp\n\nRun the plumb language server over stdio.");
                ExitCode::SUCCESS
            } else if rest.is_empty() {
                plumb::run_lsp();
                ExitCode::SUCCESS
            } else {
                eprintln!("plumb lsp: unexpected arguments");
                ExitCode::from(2)
            }
        }
        Some("help" | "--help" | "-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("plumb {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("plumb: unknown command '{command}'\n");
            print_help();
            ExitCode::from(2)
        }
        None => {
            eprintln!("plumb: command must be valid UTF-8");
            ExitCode::from(2)
        }
    }
}

fn delegated_args(name: &str, rest: Vec<OsString>) -> Vec<OsString> {
    std::iter::once(OsString::from(name)).chain(rest).collect()
}

fn wants_help(args: &[OsString]) -> bool {
    matches!(args, [argument] if argument == "--help" || argument == "-h")
}

fn print_help() {
    println!(
        "Strict plumb markup language and tooling\n\nUsage: plumb <COMMAND>\n\nCommands:\n  fmt     Format documents\n  export  Emit Pandoc JSON\n  note    Query notes\n  task    Query or update tasks\n  lsp     Run the language server\n  help    Print this help"
    );
}
