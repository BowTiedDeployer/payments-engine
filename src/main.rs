#![forbid(unsafe_code)]

use std::env;
use std::fs::File;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;

fn main() -> ExitCode {
    // Diagnostics go to stderr; stdout carries only the result CSV.
    // RUST_LOG=debug also shows each rejected transaction.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut args = env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: {} <transactions.csv>", env!("CARGO_BIN_NAME"));
        return ExitCode::from(2);
    };

    match run(Path::new(&path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &Path) -> anyhow::Result<()> {
    // No BufReader: the csv reader already buffers its input.
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    payments_engine::process(file, io::stdout().lock())
        .with_context(|| format!("failed to process {}", path.display()))
}
