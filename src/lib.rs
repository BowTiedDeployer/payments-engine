#![forbid(unsafe_code)]

pub mod account;
pub mod amount;
pub mod engine;
pub mod transaction;

use std::io;

use log::{debug, warn};
use serde::Serialize;

pub use account::Account;
pub use amount::Amount;
pub use engine::{Engine, TxError};
pub use transaction::{ClientId, Record, Transaction, TxId};

const OUTPUT_HEADER: [&str; 5] = ["client", "available", "held", "total", "locked"];

/// Reads transactions from `input` one row at a time, applies them, and
/// writes the final account balances to `output`.
///
/// Rows that can't be parsed are logged at `warn` and skipped, so one bad
/// line from a partner doesn't sink the whole batch. Transactions the engine
/// refuses (insufficient funds, unknown tx, ...) are normal outcomes and are
/// logged at `debug`. Only I/O errors stop processing: a partly read file
/// must not produce output that looks complete.
pub fn process<R: io::Read, W: io::Write>(input: R, output: W) -> Result<(), csv::Error> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        // Dispute rows may omit the trailing amount column.
        .flexible(true)
        .from_reader(input);
    let headers = reader.headers()?.clone();

    let mut engine = Engine::default();
    // One record buffer for the whole file: rows are parsed in place rather
    // than allocated one by one.
    let mut record = csv::StringRecord::new();
    loop {
        match reader.read_record(&mut record) {
            Ok(true) => {}
            Ok(false) => break,
            Err(err) if err.is_io_error() => return Err(err),
            Err(err) => {
                warn!("skipping unreadable row: {err}");
                continue;
            }
        }
        let line = record.position().map_or(0, csv::Position::line);

        let transaction = match record
            .deserialize::<Record>(Some(&headers))
            .map_err(|err| err.to_string())
            .and_then(|row| Transaction::try_from(row).map_err(|err| err.to_string()))
        {
            Ok(transaction) => transaction,
            Err(err) => {
                warn!("line {line}: skipping invalid row: {err}");
                continue;
            }
        };

        if let Err(err) = engine.apply(transaction) {
            debug!("line {line}: rejected: {err}");
        }
    }

    write_accounts(&engine, output)
}

#[derive(Serialize)]
struct AccountRow {
    client: ClientId,
    available: Amount,
    held: Amount,
    total: Amount,
    locked: bool,
}

fn write_accounts<W: io::Write>(engine: &Engine, output: W) -> Result<(), csv::Error> {
    // The header is written by hand so an input with no transactions still
    // produces a well-formed (empty) table.
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(output);
    writer.write_record(OUTPUT_HEADER)?;

    // Row order doesn't matter to consumers, but a stable order makes the
    // output diffable. There are at most 65,536 clients, so sorting is cheap.
    let mut accounts: Vec<_> = engine.accounts().collect();
    accounts.sort_unstable_by_key(|(client, _)| *client);

    for (client, account) in accounts {
        writer.serialize(AccountRow {
            client,
            available: account.available(),
            held: account.held(),
            total: account.total(),
            locked: account.is_locked(),
        })?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let mut output = Vec::new();
        process(input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn processes_a_csv_end_to_end() {
        let input = "\
type, client, tx, amount
deposit, 1, 1, 1.0
deposit, 2, 2, 2.0
deposit, 1, 3, 2.0
withdrawal, 1, 4, 1.5
withdrawal, 2, 5, 3.0
";
        assert_eq!(
            run(input),
            "\
client,available,held,total,locked
1,1.5000,0.0000,1.5000,false
2,2.0000,0.0000,2.0000,false
"
        );
    }

    #[test]
    fn empty_input_still_writes_the_header() {
        assert_eq!(run(""), "client,available,held,total,locked\n");
        assert_eq!(
            run("type,client,tx,amount\n"),
            "client,available,held,total,locked\n"
        );
    }

    #[test]
    fn invalid_rows_are_skipped() {
        let input = "\
type,client,tx,amount
deposit,1,1,1.0
deposit,1,2,lots
transfer,1,3,1.0
deposit,70000,4,1.0
deposit,1,5,1.23456
deposit,1,6
deposit,1,7,2.0
";
        assert_eq!(
            run(input),
            "\
client,available,held,total,locked
1,3.0000,0.0000,3.0000,false
"
        );
    }
}
