#![forbid(unsafe_code)]

use std::env;
use std::fs::File;
use std::io::{self, Seek};
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
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let context = || format!("failed to process {}", path.display());

    // A regular file can be read twice: first to find the disputed tx ids,
    // so the engine only stores those deposits. A pipe gets a single pass.
    let engine = if file.metadata().with_context(context)?.is_file() {
        let disputed = payments_engine::disputed_tx_ids(&file).with_context(context)?;
        file.rewind().with_context(context)?;
        payments_engine::Engine::remembering_only(disputed)
    } else {
        payments_engine::Engine::default()
    };
    payments_engine::process_with(engine, file, io::stdout().lock()).with_context(context)
}
