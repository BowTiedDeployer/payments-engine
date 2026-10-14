//! Throughput benchmarks.
//!
//! ```text
//! cargo bench
//! ```
//!
//! Both benchmarks run the same 100,000 synthetic rows. `process` is the
//! whole pipeline (CSV in, engine, CSV out). `engine` applies already parsed
//! transactions, so the gap between the two is the cost of CSV handling.

use std::hint::black_box;
use std::io;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use payments_engine::{Engine, Record, Transaction, process};

#[path = "../examples/generate.rs"]
#[allow(dead_code)] // Only `write_csv` is used here, not the example's `main`.
mod generate;

const ROWS: u32 = 100_000;

fn synthetic_csv() -> Vec<u8> {
    let mut csv = Vec::new();
    generate::write_csv(&mut csv, ROWS, &mut fastrand::Rng::with_seed(42)).unwrap();
    csv
}

fn parse(csv: &[u8]) -> Vec<Transaction> {
    csv::Reader::from_reader(csv)
        .deserialize::<Record>()
        .map(|row| Transaction::try_from(row.unwrap()).unwrap())
        .collect()
}

fn throughput(c: &mut Criterion) {
    let csv = synthetic_csv();
    let transactions = parse(&csv);

    let mut group = c.benchmark_group("100k_rows");
    group.throughput(Throughput::Elements(ROWS.into()));
    group.measurement_time(Duration::from_secs(10));
    group.bench_function("process", |b| {
        b.iter(|| process(black_box(csv.as_slice()), io::sink()).unwrap());
    });
    group.bench_function("engine", |b| {
        b.iter(|| {
            let mut engine = Engine::default();
            for &transaction in &transactions {
                // Rejections are part of the workload, not failures.
                let _ = engine.apply(black_box(transaction));
            }
            engine
        });
    });
    group.finish();
}

criterion_group!(benches, throughput);
criterion_main!(benches);
