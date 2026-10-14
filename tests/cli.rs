//! Runs the real binary the way it will be run in production:
//! `payments-engine input.csv > output.csv`.
//!
//! Every directory under `tests/fixtures` holds an `input.csv` and the
//! `expected.csv` it should produce. Adding a case means adding a directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_payments-engine");

fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(BIN)
        .args(args)
        .output()
        .expect("failed to start binary")
}

fn fixture_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut dirs: Vec<_> = fs::read_dir(&root)
        .expect("tests/fixtures exists")
        .map(|entry| entry.expect("readable fixture entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn fixtures_produce_expected_output() {
    let dirs = fixture_dirs();
    assert!(!dirs.is_empty(), "no fixtures found");

    let mut failures = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy();
        let output = run([dir.join("input.csv")]);
        let expected = fs::read_to_string(dir.join("expected.csv")).expect("expected.csv");
        let actual = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            failures.push(format!(
                "{name}: exited with {}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        } else if actual != expected {
            failures.push(format!(
                "{name}:\n--- expected\n{expected}--- actual\n{actual}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn missing_argument_prints_usage() {
    let output = run::<_, &str>([]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
}

#[test]
fn extra_arguments_are_rejected() {
    let output = run(["a.csv", "b.csv"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_file_is_reported() {
    let output = run(["does-not-exist.csv"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does-not-exist.csv"));
}
