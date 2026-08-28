use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SourceEpoch {
    AttachedV1,
    DocumentGroupV1,
    HeadSpaceV1,
    TaskEventMarkersV1,
}

#[derive(Debug, Parser)]
#[command(
    name = "plumb migrate",
    about = "Migrate an explicit plumb syntax epoch"
)]
struct Args {
    #[arg(long, value_enum)]
    from: SourceEpoch,
    #[arg(long)]
    check: bool,
    paths: Vec<PathBuf>,
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match migrate_inputs(args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("plumb migrate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn migrate_inputs(args: Args) -> Result<bool, String> {
    if args.paths.is_empty() {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        let migrated = migrate_source(args.from, &source, "stdin")?;
        if args.check {
            return Ok(migrated == source);
        }
        print!("{migrated}");
        return Ok(true);
    }

    let mut unchanged = true;
    for path in args.paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let migrated = migrate_source(args.from, &source, &path.display().to_string())?;
        if migrated == source {
            continue;
        }
        unchanged = false;
        if args.check {
            eprintln!("would migrate {}", path.display());
        } else {
            write_atomically(&path, migrated)?;
        }
    }
    Ok(!args.check || unchanged)
}

fn migrate_source(epoch: SourceEpoch, source: &str, name: &str) -> Result<String, String> {
    match epoch {
        SourceEpoch::AttachedV1 => plumb_migrate::migrate_attached_v1(source),
        SourceEpoch::DocumentGroupV1 => plumb_migrate::migrate_document_group_v1(source),
        SourceEpoch::HeadSpaceV1 => plumb_migrate::migrate_head_space_v1(source),
        SourceEpoch::TaskEventMarkersV1 => plumb_migrate::migrate_task_event_markers_v1(source),
    }
    .map_err(|error| format!("{name}: {error}"))
}

fn write_atomically(path: &Path, content: String) -> Result<(), String> {
    let permissions = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .permissions();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("cannot replace {}: missing file name", path.display()))?;
    let mut temporary = parent.join(format!(".{}.plumb-migrate", file_name.to_string_lossy()));
    let mut suffix = 0u32;
    while temporary.exists() {
        suffix += 1;
        temporary = parent.join(format!(
            ".{}.plumb-migrate-{suffix}",
            file_name.to_string_lossy()
        ));
    }
    fs::write(&temporary, content)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::set_permissions(&temporary, permissions).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "cannot preserve permissions for {}: {error}",
            path.display()
        )
    })?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot replace {}: {error}", path.display()));
    }
    Ok(())
}
