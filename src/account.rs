//! A single client's balances.
//!
//! The fields are private, so the five operations below are the only way to
//! change a balance. Each one computes the new state first and assigns it
//! only if every step succeeded: a rejected operation leaves the account
//! exactly as it was. `total` is derived from `available + held` rather than
//! stored, so the three figures can't drift apart.

use crate::Amount;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    available: Amount,
    held: Amount,
    locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("account is locked")]
    Locked,
    #[error("insufficient available funds")]
    InsufficientFunds,
    #[error("insufficient held funds")]
    InsufficientHeld,
    #[error("balance out of range")]
    Overflow,
}

impl Account {
    pub fn available(&self) -> Amount {
        self.available
    }

    pub fn held(&self) -> Amount {
        self.held
    }

    pub fn total(&self) -> Amount {
        self.available
            .checked_add(self.held)
            .expect("Account::set only stores balances whose total fits")
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn deposit(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        self.set(add(self.available, amount)?, self.held)
    }

    pub fn withdraw(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_unlocked()?;
        if self.available < amount {
            return Err(AccountError::InsufficientFunds);
        }
        self.set(sub(self.available, amount)?, self.held)
    }

    /// Moves funds from available to held for a dispute.
    ///
    /// This deliberately does not require `available >= amount`. If the
    /// disputed deposit has already been withdrawn, refusing the hold would
    /// let the client keep money that is being clawed back. Available goes
    /// negative instead, which records what the client owes.
    pub fn hold(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.set(sub(self.available, amount)?, add(self.held, amount)?)
    }

    /// Returns held funds to available when a dispute is resolved.
    pub fn release(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_held(amount)?;
        self.set(add(self.available, amount)?, sub(self.held, amount)?)
    }

    /// Removes held funds for good and locks the account.
    pub fn charge_back(&mut self, amount: Amount) -> Result<(), AccountError> {
        self.ensure_held(amount)?;
        self.set(self.available, sub(self.held, amount)?)?;
        self.locked = true;
        Ok(())
    }

    fn ensure_unlocked(&self) -> Result<(), AccountError> {
        if self.locked {
            Err(AccountError::Locked)
        } else {
            Ok(())
        }
    }

    fn ensure_held(&self, amount: Amount) -> Result<(), AccountError> {
        if self.held < amount {
            Err(AccountError::InsufficientHeld)
        } else {
            Ok(())
        }
    }

    fn set(&mut self, available: Amount, held: Amount) -> Result<(), AccountError> {
        add(available, held)?;
        self.available = available;
        self.held = held;
        Ok(())
    }
}

fn add(a: Amount, b: Amount) -> Result<Amount, AccountError> {
    a.checked_add(b).ok_or(AccountError::Overflow)
}

fn sub(a: Amount, b: Amount) -> Result<Amount, AccountError> {
    a.checked_sub(b).ok_or(AccountError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amt(s: &str) -> Amount {
        s.parse().unwrap()
    }

    fn account(available: &str, held: &str) -> Account {
        Account {
            available: amt(available),
            held: amt(held),
            locked: false,
        }
    }

    #[test]
    fn deposit_increases_available_and_total() {
        let mut acc = Account::default();
        acc.deposit(amt("1.5")).unwrap();
        assert_eq!(acc.available(), amt("1.5"));
        assert_eq!(acc.held(), Amount::ZERO);
        assert_eq!(acc.total(), amt("1.5"));
    }

    #[test]
    fn withdraw_decreases_available_and_total() {
        let mut acc = account("3", "0");
        acc.withdraw(amt("1")).unwrap();
        assert_eq!(acc.available(), amt("2"));
        assert_eq!(acc.total(), amt("2"));
    }

    #[test]
    fn withdraw_can_empty_the_account() {
        let mut acc = account("3", "0");
        acc.withdraw(amt("3")).unwrap();
        assert_eq!(acc.total(), Amount::ZERO);
    }

    #[test]
    fn withdraw_more_than_available_leaves_account_unchanged() {
        let mut acc = account("1", "5");
        let before = acc.clone();
        assert_eq!(acc.withdraw(amt("2")), Err(AccountError::InsufficientFunds));
        assert_eq!(acc, before);
    }

    #[test]
    fn hold_moves_funds_from_available_to_held() {
        let mut acc = account("5", "0");
        acc.hold(amt("2")).unwrap();
        assert_eq!(acc.available(), amt("3"));
        assert_eq!(acc.held(), amt("2"));
        assert_eq!(acc.total(), amt("5"));
    }

    #[test]
    fn hold_can_take_available_below_zero() {
        let mut acc = account("1", "0");
        acc.hold(amt("3")).unwrap();
        assert_eq!(acc.available(), amt("-2"));
        assert_eq!(acc.held(), amt("3"));
        assert_eq!(acc.total(), amt("1"));
    }

    #[test]
    fn release_moves_funds_from_held_to_available() {
        let mut acc = account("3", "2");
        acc.release(amt("2")).unwrap();
        assert_eq!(acc.available(), amt("5"));
        assert_eq!(acc.held(), Amount::ZERO);
    }

    #[test]
    fn charge_back_removes_held_funds_and_locks() {
        let mut acc = account("3", "2");
        acc.charge_back(amt("2")).unwrap();
        assert_eq!(acc.available(), amt("3"));
        assert_eq!(acc.held(), Amount::ZERO);
        assert_eq!(acc.total(), amt("3"));
        assert!(acc.is_locked());
    }

    #[test]
    fn release_and_charge_back_cannot_exceed_held() {
        let mut acc = account("3", "1");
        let before = acc.clone();
        assert_eq!(acc.release(amt("2")), Err(AccountError::InsufficientHeld));
        assert_eq!(
            acc.charge_back(amt("2")),
            Err(AccountError::InsufficientHeld)
        );
        assert_eq!(acc, before);
    }

    #[test]
    fn locked_account_rejects_deposits_and_withdrawals() {
        let mut acc = account("3", "2");
        acc.charge_back(amt("1")).unwrap();
        let before = acc.clone();
        assert_eq!(acc.deposit(amt("1")), Err(AccountError::Locked));
        assert_eq!(acc.withdraw(amt("1")), Err(AccountError::Locked));
        assert_eq!(acc, before);
    }

    #[test]
    fn locked_account_still_settles_open_disputes() {
        let mut acc = account("3", "2");
        acc.charge_back(amt("1")).unwrap();
        acc.release(amt("1")).unwrap();
        acc.hold(amt("1")).unwrap();
        assert_eq!(acc.available(), amt("3"));
        assert_eq!(acc.held(), amt("1"));
    }

    #[test]
    fn overflow_is_rejected_and_leaves_account_unchanged() {
        let max = "922337203685477.5807";
        let mut acc = account(max, "0");
        let before = acc.clone();
        assert_eq!(acc.deposit(amt("0.0001")), Err(AccountError::Overflow));
        assert_eq!(acc, before);

        // available + held must fit too, not just each field on its own.
        let mut acc = account("922337203685477", "0.5807");
        let before = acc.clone();
        assert_eq!(acc.deposit(amt("0.0001")), Err(AccountError::Overflow));
        assert_eq!(acc, before);
    }
}
