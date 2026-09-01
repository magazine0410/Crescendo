use anyhow::{bail, Context, Result};
use crescendo_core::plugins::tooling::{
    pack_plugin_source, validate_plugin_package, validate_plugin_source, MANIFEST_V1_SCHEMA,
};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        print_usage();
        bail!("missing command");
    };
    match command.to_str() {
        Some("validate") => {
            let input = arguments
                .next()
                .context("validate needs a source directory or package")?;
            ensure_no_more(arguments)?;
            let input = PathBuf::from(input);
            if input.is_dir() {
                let report = validate_plugin_source(&input)?;
                println!(
                    "valid source: {} {} ({} files, {} bytes)",
                    report.manifest.id,
                    report.manifest.version,
                    report.entry_count,
                    report.extracted_bytes
                );
            } else {
                let report = validate_plugin_package(&input)?;
                println!(
                    "valid package: {} {} ({} files, {} extracted bytes, {} archive bytes)",
                    report.manifest.id,
                    report.manifest.version,
                    report.entry_count,
                    report.extracted_bytes,
                    report.archive_bytes
                );
                println!("SHA-256: {}", report.sha256);
            }
        }
        Some("pack") => {
            let source = PathBuf::from(arguments.next().context("pack needs a source directory")?);
            let requested_output = arguments.next().map(PathBuf::from);
            ensure_no_more(arguments)?;
            let source_report = validate_plugin_source(&source)?;
            let output = requested_output.unwrap_or_else(|| {
                PathBuf::from(format!(
                    "{}-{}.crescendo-plugin",
                    source_report.manifest.id, source_report.manifest.version
                ))
            });
            let report = pack_plugin_source(&source, &output)?;
            println!("created: {}", absolute_for_display(&output).display());
            println!("plugin: {} {}", report.manifest.id, report.manifest.version);
            println!("SHA-256: {}", report.sha256);
        }
        Some("schema") => {
            ensure_no_more(arguments)?;
            print!("{MANIFEST_V1_SCHEMA}");
        }
        Some("help") | Some("--help") | Some("-h") => {
            ensure_no_more(arguments)?;
            print_usage();
        }
        _ => {
            print_usage();
            bail!("unknown command");
        }
    }
    Ok(())
}

fn ensure_no_more(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<()> {
    if arguments.next().is_some() {
        bail!("too many arguments");
    }
    Ok(())
}

fn absolute_for_display(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn print_usage() {
    eprintln!(
        "Crescendo plugin package tooling\n\n\
         Usage:\n  \
           crescendo-plugin validate <source-dir|package.crescendo-plugin>\n  \
           crescendo-plugin pack <source-dir> [output.crescendo-plugin]\n  \
           crescendo-plugin schema"
    );
}
