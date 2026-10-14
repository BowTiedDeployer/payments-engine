//! Writes a synthetic transactions CSV for load testing.
//!
//! ```text
//! mkdir -p data && cargo run --release --example generate -- 10000000 42 > data/bench.csv
//! ```
//!
//! Arguments: the number of rows (default 1,000,000) and an optional seed.
//! About 50% of rows are deposits, 44% withdrawals, 4% disputes, 1.5%
//! resolves and 0.5% chargebacks, across all 65,536 clients.
//!
//! Nothing is kept per row, so memory stays constant at any size. Tx ids are
//! a bijective hash of a counter: unique, unordered and spread over the whole
//! u32 range. Whether an id is a deposit, and its client, is also a hash of
//! the id, so a dispute can pick any earlier id without a lookup.

use std::env;
use std::io::{self, BufWriter, Write};

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let rows = match args.next() {
        Some(n) => n.parse().expect("rows must be a number up to 4294967295"),
        None => 1_000_000,
    };
    let mut rng = match args.next().and_then(|seed| seed.parse().ok()) {
        Some(seed) => fastrand::Rng::with_seed(seed),
        None => fastrand::Rng::new(),
    };

    write_csv(BufWriter::new(io::stdout().lock()), rows, &mut rng)
}

/// Writes a header and `rows` transactions to `out`. There are never more
/// rows than u32 ids, so every deposit and withdrawal gets its own id.
pub fn write_csv<W: Write>(mut out: W, rows: u32, rng: &mut fastrand::Rng) -> io::Result<()> {
    writeln!(out, "type,client,tx,amount")?;
    let ids = Ids {
        order_key: rng.u32(..),
        kind_key: rng.u32(..),
    };

    // How many tx ids have been handed out so far.
    let mut used: u32 = 0;
    let mut written: u32 = 0;
    while written < rows {
        let roll = rng.u32(0..1000);
        if roll >= 940 {
            if let Some((tx, client)) = ids.earlier_deposit(used, rng) {
                let kind = match roll {
                    940..=979 => "dispute",
                    980..=994 => "resolve",
                    _ => "chargeback",
                };
                writeln!(out, "{kind},{client},{tx},")?;
                written += 1;
            }
            continue;
        }

        let tx = ids.nth(used);
        used += 1;
        let kind = if ids.is_deposit(tx) {
            "deposit"
        } else {
            "withdrawal"
        };
        let units = rng.u64(1..=10_000_000);
        writeln!(
            out,
            "{kind},{},{tx},{}.{:04}",
            ids.client(tx),
            units / 10_000,
            units % 10_000
        )?;
        written += 1;
    }
    out.flush()
}

/// Derives everything about a tx id from the id itself.
struct Ids {
    order_key: u32,
    kind_key: u32,
}

impl Ids {
    /// The `n`th id handed out. Distinct `n` give distinct ids.
    fn nth(&self, n: u32) -> u32 {
        mix(n ^ self.order_key)
    }

    fn is_deposit(&self, tx: u32) -> bool {
        // 500 of the 940 non-dispute rows per thousand are deposits.
        mix(tx ^ self.kind_key) % 940 < 500
    }

    fn client(&self, tx: u32) -> u16 {
        (mix(tx ^ self.kind_key) >> 16) as u16
    }

    /// A random deposit among the first `used` ids, if one turns up quickly.
    fn earlier_deposit(&self, used: u32, rng: &mut fastrand::Rng) -> Option<(u32, u16)> {
        if used == 0 {
            return None;
        }
        (0..16)
            .map(|_| self.nth(rng.u32(..used)))
            .find(|&tx| self.is_deposit(tx))
            .map(|tx| (tx, self.client(tx)))
    }
}

/// A bijective 32-bit hash (lowbias32). Xor-shifts and multiplication by an
/// odd constant are each invertible, so no two inputs share an output.
fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}
