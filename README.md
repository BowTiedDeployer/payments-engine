# payments-engine

A toy payments engine. It reads a CSV of transactions, applies deposits, withdrawals, disputes, resolutions and chargebacks to client accounts, and writes the final balances as CSV.

```
cargo run -- transactions.csv > accounts.csv
```

Only the result goes to stdout. Malformed rows are logged to stderr at `warn`; rejected transactions (insufficient funds, unknown tx, ...) at `debug`:

```
RUST_LOG=debug cargo run -- transactions.csv > accounts.csv
```

## Layout

```
src/amount.rs       fixed-point money type
src/transaction.rs  ids, the raw CSV Record and the validated Transaction
src/account.rs      one client's balances and the operations that change them
src/engine.rs       accounts, stored deposits, the dispute state machine
src/tx_ids.rs       set of used tx ids with a 512 MiB ceiling
src/lib.rs          streams CSV in, applies it, writes CSV out
src/main.rs         arguments, one or two passes, exit codes
examples/generate.rs  synthetic input for load testing
benches/process.rs  throughput benchmarks
```

Each layer only knows the one below it: `Account` knows nothing about tx ids, `Engine` nothing about CSV, `main` nothing about the rules.

## Assumptions

Where the spec leaves a choice open, I did what a bank would do.

- **Only deposits can be disputed.** Disputing a withdrawal would mean holding money that has already left. Disputes pointing at a withdrawal are ignored.
- **A dispute can take `available` negative.** Deposit, withdraw, then dispute is the fraud case from the brief; refusing the dispute would let the client keep the money.
- **Dispute, resolve and chargeback rows must name the deposit's client**, otherwise they're ignored, so one client can't freeze another's funds.
- **Lifecycle per deposit:** `Settled -> Disputed -> Settled` (resolve) or `ChargedBack` (final). A resolved deposit can be disputed again.
- **A locked account refuses deposits and withdrawals**, but open disputes on it still settle.
- **Reused tx ids are rejected, whatever the type.** A repeat is most likely a replayed row, and accepting it could debit a client twice. A refused transaction doesn't take its id.
- **Rejected transactions leave no trace** and can't be disputed later. A failed deposit or withdrawal still creates the account, with zero balances.
- **Amounts must be positive with at most four decimals.** More precision is rejected, not rounded.
- **Bad rows are skipped.** Only I/O errors stop processing, so a partly read file never produces output that looks complete.

## Correctness

**Types do part of the work.** `Amount` is an `i64` of ten-thousandths with checked arithmetic (no floats). `ClientId` and `TxId` are newtypes. The raw CSV `Record` is validated once into a `Transaction`, where a deposit always has an amount and a dispute never does. `Account` fields are private and `total` is computed, so balances can't drift apart. The dispute state machine is one exhaustive `match`.

**Tests:**
- Unit tests for each module, one per rule. Every rejection also checks nothing changed.
- End-to-end tests (`tests/cli.rs`) run the binary on each `tests/fixtures/<case>/input.csv` and compare against a hand-computed `expected.csv`. Each case runs both as a file and piped.
- Property tests (`tests/invariants.rs`) run 2,000 random sequences and check after every row that held is never negative, totals add up, rejected rows change nothing, no id is used twice and locked accounts never gain. A second one checks that storing only disputed deposits gives the same results. I planted bugs to confirm both can fail.

```
cargo test
```

## Safety and error handling

- `#![forbid(unsafe_code)]`, and no `unwrap` on anything that comes from the input.
- Typed errors in the library (`thiserror`), `anyhow` only in `main`.
- Exit codes: 0 on success, 2 for usage errors, 1 if the file can't be read.

## Efficiency

Input is streamed one row at a time, so the file is never held in memory. What grows is what the engine has to remember:

- **Accounts:** at most 65,536.
- **Used tx ids**, to refuse replays: a hash set that switches to a 512 MiB bitset at about 16.8 million ids, so it has a ceiling.
- **Deposits**, which a later dispute may need: about 30 to 60 bytes each. This is the real cost.

**Two passes for files.** A stored deposit is only read back once a dispute names it. So for a regular file, `main` first collects the ids of dispute rows, then the engine stores only those deposits. Piped input can only be read once and keeps a single pass.

Measured on an Apple M1 Max with 10 million generated rows (330 MB):

| Input | Time | Peak memory |
|---|---|---|
| File (two passes) | 8.5 s | 205 MB |
| Piped (one pass) | 6.2 s | 430 MB |

```
mkdir -p data && cargo run --release --example generate -- 10000000 42 > data/bench.csv
cargo build --release && /usr/bin/time -l target/release/payments-engine data/bench.csv > /dev/null
cargo bench --bench process
```

The benchmark shows about 85% of the time is CSV parsing, not the ledger.

**Limits.** Tx ids are `u32`, so a file can hold up to 2^32 transactions (about 150 GB of CSV). In two-pass mode that's an estimated 6 to 15 GB of memory, but piped input, or a file where nearly every deposit is disputed, would need more memory than most machines have. The next step would be to move deposits behind a small trait and store them in an embedded key-value store such as `redb`, keeping the id set in memory. I haven't built it: it adds a dependency and failure modes no input in the brief needs.

## Running this behind a server

`Engine` is synchronous, owns its state and knows nothing about CSV, so it stays as it is. Every transaction touches exactly one client, so `ClientId` is a natural shard key: N workers each own an `Engine`, and each connection sends rows to worker `client % N` over a bounded channel (backpressure for free, per-client order preserved, no shared locks).

Two things change. Detecting a reused tx id across shards would need a shared id set. And a stream can't be read twice, so each worker stores every deposit; a long-running server would need the on-disk store above, or to drop deposits once they're past any dispute window.

## What I'd do next

- Fuzz `Amount::from_str` and `process` with `cargo fuzz`.
- Print a summary of rejections by reason at the end of a run.
- Add the on-disk deposit store.
