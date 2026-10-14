//! Input types.
//!
//! [`Record`] mirrors a CSV row, where every column except `type` is just a
//! value and `amount` may be missing. [`Transaction`] is what the engine
//! works with: a deposit always has an amount, a dispute never does. Rows
//! are checked once, in `TryFrom<Record>`, so the engine never has to.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Amount;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ClientId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct TxId(pub u32);

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// One row of the input file, as written.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Record {
    #[serde(rename = "type")]
    pub kind: Kind,
    pub client: ClientId,
    pub tx: TxId,
    /// Dispute rows may leave the column empty or drop it entirely
    /// (`dispute,1,1`), so a missing column is treated as no amount.
    #[serde(default)]
    pub amount: Option<Amount>,
}

/// A validated transaction. Amounts on deposits and withdrawals are always
/// greater than zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transaction {
    Deposit {
        client: ClientId,
        tx: TxId,
        amount: Amount,
    },
    Withdrawal {
        client: ClientId,
        tx: TxId,
        amount: Amount,
    },
    Dispute {
        client: ClientId,
        tx: TxId,
    },
    Resolve {
        client: ClientId,
        tx: TxId,
    },
    Chargeback {
        client: ClientId,
        tx: TxId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidRecord {
    #[error("tx {0}: missing amount")]
    MissingAmount(TxId),
    #[error("tx {0}: amount must be greater than zero")]
    NonPositiveAmount(TxId),
}

impl TryFrom<Record> for Transaction {
    type Error = InvalidRecord;

    fn try_from(record: Record) -> Result<Self, Self::Error> {
        let Record {
            kind,
            client,
            tx,
            amount,
        } = record;

        let positive_amount = || match amount {
            None => Err(InvalidRecord::MissingAmount(tx)),
            Some(a) if !a.is_positive() => Err(InvalidRecord::NonPositiveAmount(tx)),
            Some(a) => Ok(a),
        };

        // An amount on a dispute, resolve or chargeback is ignored: it isn't
        // used, and rejecting the row over it would drop a valid dispute.
        Ok(match kind {
            Kind::Deposit => Transaction::Deposit {
                client,
                tx,
                amount: positive_amount()?,
            },
            Kind::Withdrawal => Transaction::Withdrawal {
                client,
                tx,
                amount: positive_amount()?,
            },
            Kind::Dispute => Transaction::Dispute { client, tx },
            Kind::Resolve => Transaction::Resolve { client, tx },
            Kind::Chargeback => Transaction::Chargeback { client, tx },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: Kind, amount: Option<&str>) -> Record {
        Record {
            kind,
            client: ClientId(1),
            tx: TxId(7),
            amount: amount.map(|a| a.parse().unwrap()),
        }
    }

    #[test]
    fn deposit_and_withdrawal_carry_their_amount() {
        let amount = "1.5".parse().unwrap();
        assert_eq!(
            Transaction::try_from(record(Kind::Deposit, Some("1.5"))),
            Ok(Transaction::Deposit {
                client: ClientId(1),
                tx: TxId(7),
                amount
            })
        );
        assert_eq!(
            Transaction::try_from(record(Kind::Withdrawal, Some("1.5"))),
            Ok(Transaction::Withdrawal {
                client: ClientId(1),
                tx: TxId(7),
                amount
            })
        );
    }

    #[test]
    fn deposit_and_withdrawal_require_an_amount() {
        for kind in [Kind::Deposit, Kind::Withdrawal] {
            assert_eq!(
                Transaction::try_from(record(kind, None)),
                Err(InvalidRecord::MissingAmount(TxId(7)))
            );
        }
    }

    #[test]
    fn deposit_and_withdrawal_reject_zero_and_negative_amounts() {
        for kind in [Kind::Deposit, Kind::Withdrawal] {
            for amount in ["0", "0.0000", "-1"] {
                assert_eq!(
                    Transaction::try_from(record(kind, Some(amount))),
                    Err(InvalidRecord::NonPositiveAmount(TxId(7)))
                );
            }
        }
    }

    #[test]
    fn dispute_family_ignores_any_amount() {
        let client = ClientId(1);
        let tx = TxId(7);
        let cases = [
            (Kind::Dispute, Transaction::Dispute { client, tx }),
            (Kind::Resolve, Transaction::Resolve { client, tx }),
            (Kind::Chargeback, Transaction::Chargeback { client, tx }),
        ];
        for (kind, expected) in cases {
            assert_eq!(Transaction::try_from(record(kind, None)), Ok(expected));
            assert_eq!(Transaction::try_from(record(kind, Some("5"))), Ok(expected));
        }
    }
}
