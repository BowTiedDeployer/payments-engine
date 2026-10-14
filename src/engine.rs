//! Applies transactions to accounts.
//!
//! The engine owns every account and the deposits that may still be
//! disputed. It never logs or prints: each rejected transaction comes back
//! as a [`TxError`] and the caller decides what to do with it.

use std::collections::{HashMap, HashSet};

use crate::account::AccountError;
use crate::{Account, Amount, ClientId, Transaction, TxId};

#[derive(Debug, Default)]
pub struct Engine {
    accounts: HashMap<ClientId, Account>,
    /// Every accepted deposit and withdrawal, so a reused id can be refused.
    used_ids: HashSet<TxId>,
    /// Deposits that a later dispute may refer to.
    deposits: HashMap<TxId, Deposit>,
}

#[derive(Debug)]
struct Deposit {
    amount: Amount,
    client: ClientId,
    state: DepositState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepositState {
    Settled,
    Disputed,
    ChargedBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TxError {
    #[error("client {client}: {source}")]
    Account {
        client: ClientId,
        source: AccountError,
    },
    #[error("tx {0} already exists")]
    DuplicateTx(TxId),
    #[error("tx {0} is not a known deposit")]
    UnknownTx(TxId),
    #[error("tx {tx} belongs to client {owner}, not client {client}")]
    ClientMismatch {
        tx: TxId,
        owner: ClientId,
        client: ClientId,
    },
    #[error("tx {0} is already under dispute")]
    AlreadyDisputed(TxId),
    #[error("tx {0} is not under dispute")]
    NotDisputed(TxId),
    #[error("tx {0} has been charged back")]
    ChargedBack(TxId),
}

/// The three rows that move a deposit through its dispute lifecycle.
#[derive(Debug, Clone, Copy)]
enum DisputeStep {
    Dispute,
    Resolve,
    Chargeback,
}

impl Engine {
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), TxError> {
        match transaction {
            Transaction::Deposit { client, tx, amount } => self.deposit(client, tx, amount),
            Transaction::Withdrawal { client, tx, amount } => self.withdraw(client, tx, amount),
            Transaction::Dispute { client, tx } => self.step(client, tx, DisputeStep::Dispute),
            Transaction::Resolve { client, tx } => self.step(client, tx, DisputeStep::Resolve),
            Transaction::Chargeback { client, tx } => {
                self.step(client, tx, DisputeStep::Chargeback)
            }
        }
    }

    /// Every account seen so far, in no particular order.
    pub fn accounts(&self) -> impl Iterator<Item = (ClientId, &Account)> {
        self.accounts.iter().map(|(id, account)| (*id, account))
    }

    fn deposit(&mut self, client: ClientId, tx: TxId, amount: Amount) -> Result<(), TxError> {
        let account = self.accounts.entry(client).or_default();
        // Tx ids are unique. A reused one is most likely a replayed row, and
        // as a deposit it would also overwrite the original's dispute state.
        if self.used_ids.contains(&tx) {
            return Err(TxError::DuplicateTx(tx));
        }
        account
            .deposit(amount)
            .map_err(|source| TxError::Account { client, source })?;
        // Recorded only once the account has accepted it: a refused deposit
        // never happened and can't be disputed later.
        self.used_ids.insert(tx);
        self.deposits.insert(
            tx,
            Deposit {
                amount,
                client,
                state: DepositState::Settled,
            },
        );
        Ok(())
    }

    fn withdraw(&mut self, client: ClientId, tx: TxId, amount: Amount) -> Result<(), TxError> {
        let account = self.accounts.entry(client).or_default();
        // A replayed withdrawal would debit the client twice.
        if self.used_ids.contains(&tx) {
            return Err(TxError::DuplicateTx(tx));
        }
        account
            .withdraw(amount)
            .map_err(|source| TxError::Account { client, source })?;
        // Like deposits, a refused withdrawal never happened, so its id isn't
        // taken.
        self.used_ids.insert(tx);
        Ok(())
    }

    /// Moves a deposit through `Settled -> Disputed -> Settled | ChargedBack`.
    ///
    /// Every (step, state) pair is listed in one match, so the compiler
    /// checks that no combination is left unhandled. The deposit's state
    /// changes only after the account has accepted the balance change.
    fn step(&mut self, client: ClientId, tx: TxId, step: DisputeStep) -> Result<(), TxError> {
        let deposit = self.deposits.get_mut(&tx).ok_or(TxError::UnknownTx(tx))?;
        // A row may only act on the client's own deposit. Otherwise one
        // client's partner could freeze another client's funds.
        if deposit.client != client {
            return Err(TxError::ClientMismatch {
                tx,
                owner: deposit.client,
                client,
            });
        }
        let account = self
            .accounts
            .get_mut(&client)
            .expect("every stored deposit was accepted by an existing account");
        let amount = deposit.amount;
        let to_err = |source| TxError::Account { client, source };

        deposit.state = match (step, deposit.state) {
            (DisputeStep::Dispute, DepositState::Settled) => {
                account.hold(amount).map_err(to_err)?;
                DepositState::Disputed
            }
            (DisputeStep::Resolve, DepositState::Disputed) => {
                account.release(amount).map_err(to_err)?;
                DepositState::Settled
            }
            (DisputeStep::Chargeback, DepositState::Disputed) => {
                account.charge_back(amount).map_err(to_err)?;
                DepositState::ChargedBack
            }
            (DisputeStep::Dispute, DepositState::Disputed) => {
                return Err(TxError::AlreadyDisputed(tx));
            }
            (DisputeStep::Resolve | DisputeStep::Chargeback, DepositState::Settled) => {
                return Err(TxError::NotDisputed(tx));
            }
            (_, DepositState::ChargedBack) => return Err(TxError::ChargedBack(tx)),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amt(s: &str) -> Amount {
        s.parse().unwrap()
    }

    fn deposit(client: u16, tx: u32, amount: &str) -> Transaction {
        Transaction::Deposit {
            client: ClientId(client),
            tx: TxId(tx),
            amount: amt(amount),
        }
    }

    fn withdrawal(client: u16, tx: u32, amount: &str) -> Transaction {
        Transaction::Withdrawal {
            client: ClientId(client),
            tx: TxId(tx),
            amount: amt(amount),
        }
    }

    fn dispute(client: u16, tx: u32) -> Transaction {
        Transaction::Dispute {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    fn resolve(client: u16, tx: u32) -> Transaction {
        Transaction::Resolve {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    fn chargeback(client: u16, tx: u32) -> Transaction {
        Transaction::Chargeback {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    /// (available, held, total, locked) as strings, so expectations read
    /// like the output file.
    fn balances(engine: &Engine, client: u16) -> (String, String, String, bool) {
        let account = &engine.accounts[&ClientId(client)];
        (
            account.available().to_string(),
            account.held().to_string(),
            account.total().to_string(),
            account.is_locked(),
        )
    }

    fn row(
        available: &str,
        held: &str,
        total: &str,
        locked: bool,
    ) -> (String, String, String, bool) {
        (available.into(), held.into(), total.into(), locked)
    }

    #[test]
    fn processes_deposits_and_withdrawals() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "1.0")).unwrap();
        engine.apply(deposit(2, 2, "2.0")).unwrap();
        engine.apply(deposit(1, 3, "2.0")).unwrap();
        engine.apply(withdrawal(1, 4, "1.5")).unwrap();
        assert!(engine.apply(withdrawal(2, 5, "3.0")).is_err());

        assert_eq!(
            balances(&engine, 1),
            row("1.5000", "0.0000", "1.5000", false)
        );
        assert_eq!(
            balances(&engine, 2),
            row("2.0000", "0.0000", "2.0000", false)
        );
    }

    #[test]
    fn insufficient_funds_is_reported() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "1")).unwrap();
        assert_eq!(
            engine.apply(withdrawal(1, 2, "1.0001")),
            Err(TxError::Account {
                client: ClientId(1),
                source: AccountError::InsufficientFunds
            })
        );
        assert_eq!(
            balances(&engine, 1),
            row("1.0000", "0.0000", "1.0000", false)
        );
    }

    #[test]
    fn failed_withdrawal_still_creates_the_account() {
        let mut engine = Engine::default();
        assert!(engine.apply(withdrawal(9, 1, "1")).is_err());
        assert_eq!(
            balances(&engine, 9),
            row("0.0000", "0.0000", "0.0000", false)
        );
    }

    #[test]
    fn duplicate_deposit_id_is_rejected() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "1")).unwrap();
        assert_eq!(
            engine.apply(deposit(1, 1, "5")),
            Err(TxError::DuplicateTx(TxId(1)))
        );
        assert_eq!(
            engine.apply(deposit(2, 1, "5")),
            Err(TxError::DuplicateTx(TxId(1)))
        );
        assert_eq!(
            balances(&engine, 1),
            row("1.0000", "0.0000", "1.0000", false)
        );
        assert_eq!(
            balances(&engine, 2),
            row("0.0000", "0.0000", "0.0000", false)
        );
    }

    #[test]
    fn duplicate_withdrawal_id_is_rejected() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(withdrawal(1, 2, "3")).unwrap();
        assert_eq!(
            engine.apply(withdrawal(1, 2, "3")),
            Err(TxError::DuplicateTx(TxId(2)))
        );
        // Ids are shared across types, in both directions.
        assert_eq!(
            engine.apply(withdrawal(1, 1, "1")),
            Err(TxError::DuplicateTx(TxId(1)))
        );
        assert_eq!(
            engine.apply(deposit(1, 2, "1")),
            Err(TxError::DuplicateTx(TxId(2)))
        );
        assert_eq!(
            balances(&engine, 1),
            row("7.0000", "0.0000", "7.0000", false)
        );
    }

    #[test]
    fn a_refused_withdrawal_does_not_take_its_id() {
        let mut engine = Engine::default();
        assert!(engine.apply(withdrawal(1, 1, "5")).is_err());
        engine.apply(deposit(1, 2, "10")).unwrap();
        engine.apply(withdrawal(1, 1, "5")).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("5.0000", "0.0000", "5.0000", false)
        );
    }

    #[test]
    fn dispute_holds_the_deposited_amount() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(deposit(1, 2, "5")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("5.0000", "10.0000", "15.0000", false)
        );
    }

    #[test]
    fn resolve_releases_the_hold() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(resolve(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
    }

    #[test]
    fn chargeback_removes_the_funds_and_locks_the_account() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(deposit(1, 2, "5")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(chargeback(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("5.0000", "0.0000", "5.0000", true)
        );
    }

    #[test]
    fn deposit_withdraw_then_reverse_leaves_a_negative_balance() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(withdrawal(1, 2, "10")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("-10.0000", "10.0000", "0.0000", false)
        );
        engine.apply(chargeback(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("-10.0000", "0.0000", "-10.0000", true)
        );
    }

    #[test]
    fn locked_account_rejects_deposits_and_withdrawals() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(deposit(1, 2, "5")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(chargeback(1, 1)).unwrap();

        let locked = Err(TxError::Account {
            client: ClientId(1),
            source: AccountError::Locked,
        });
        assert_eq!(engine.apply(deposit(1, 3, "1")), locked);
        assert_eq!(engine.apply(withdrawal(1, 4, "1")), locked);
        assert_eq!(
            balances(&engine, 1),
            row("5.0000", "0.0000", "5.0000", true)
        );

        // The refused deposit was never recorded, so it can't be disputed.
        assert_eq!(
            engine.apply(dispute(1, 3)),
            Err(TxError::UnknownTx(TxId(3)))
        );
    }

    #[test]
    fn locked_account_still_settles_disputes() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(deposit(1, 2, "5")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(dispute(1, 2)).unwrap();
        engine.apply(chargeback(1, 1)).unwrap();
        engine.apply(resolve(1, 2)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("5.0000", "0.0000", "5.0000", true)
        );
    }

    #[test]
    fn dispute_rows_for_unknown_transactions_are_ignored() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(withdrawal(1, 2, "3")).unwrap();

        // tx 2 exists but is a withdrawal, which can't be disputed.
        for tx in [2, 99] {
            let unknown = Err(TxError::UnknownTx(TxId(tx)));
            assert_eq!(engine.apply(dispute(1, tx)), unknown);
            assert_eq!(engine.apply(resolve(1, tx)), unknown);
            assert_eq!(engine.apply(chargeback(1, tx)), unknown);
        }
        assert_eq!(
            balances(&engine, 1),
            row("7.0000", "0.0000", "7.0000", false)
        );
    }

    #[test]
    fn dispute_rows_must_name_the_depositing_client() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(deposit(2, 2, "10")).unwrap();

        let mismatch = Err(TxError::ClientMismatch {
            tx: TxId(1),
            owner: ClientId(1),
            client: ClientId(2),
        });
        assert_eq!(engine.apply(dispute(2, 1)), mismatch);

        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(engine.apply(resolve(2, 1)), mismatch);
        assert_eq!(engine.apply(chargeback(2, 1)), mismatch);

        assert_eq!(
            balances(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
        assert_eq!(
            balances(&engine, 2),
            row("10.0000", "0.0000", "10.0000", false)
        );
    }

    #[test]
    fn resolve_and_chargeback_need_an_open_dispute() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        assert_eq!(
            engine.apply(resolve(1, 1)),
            Err(TxError::NotDisputed(TxId(1)))
        );
        assert_eq!(
            engine.apply(chargeback(1, 1)),
            Err(TxError::NotDisputed(TxId(1)))
        );
        assert_eq!(
            balances(&engine, 1),
            row("10.0000", "0.0000", "10.0000", false)
        );
    }

    #[test]
    fn a_deposit_cannot_be_disputed_twice_at_once() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(
            engine.apply(dispute(1, 1)),
            Err(TxError::AlreadyDisputed(TxId(1)))
        );
        assert_eq!(
            balances(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
    }

    #[test]
    fn a_resolved_deposit_can_be_disputed_again() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(resolve(1, 1)).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(
            balances(&engine, 1),
            row("0.0000", "10.0000", "10.0000", false)
        );
    }

    #[test]
    fn a_charged_back_deposit_is_final() {
        let mut engine = Engine::default();
        engine.apply(deposit(1, 1, "10")).unwrap();
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(chargeback(1, 1)).unwrap();
        for row_after in [dispute(1, 1), resolve(1, 1), chargeback(1, 1)] {
            assert_eq!(engine.apply(row_after), Err(TxError::ChargedBack(TxId(1))));
        }
        assert_eq!(
            balances(&engine, 1),
            row("0.0000", "0.0000", "0.0000", true)
        );
    }
}
