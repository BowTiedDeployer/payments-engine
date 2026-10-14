//! Property tests: random transaction sequences must never break the
//! engine's balance invariants, and remembering only disputed deposits must
//! never change the result.
//!
//! The id space is kept small (3 clients, 20 tx ids) so that disputes,
//! resolves and chargebacks usually hit a real deposit, and duplicate ids and
//! cross-client rows come up often.

use std::collections::{BTreeMap, HashMap, HashSet};

use payments_engine::{Account, Amount, ClientId, Engine, Transaction, TxId, TxIdSet};
use proptest::prelude::*;

fn amount() -> impl Strategy<Value = Amount> {
    // 0.0001 to 100.0000, so the four decimal places are exercised.
    (1u32..=1_000_000).prop_map(|units| {
        format!("{}.{:04}", units / 10_000, units % 10_000)
            .parse()
            .unwrap()
    })
}

fn transaction() -> impl Strategy<Value = Transaction> {
    let client = (1u16..=3).prop_map(ClientId);
    let tx = (1u32..=20).prop_map(TxId);
    prop_oneof![
        3 => (client.clone(), tx.clone(), amount())
            .prop_map(|(client, tx, amount)| Transaction::Deposit { client, tx, amount }),
        2 => (client.clone(), tx.clone(), amount())
            .prop_map(|(client, tx, amount)| Transaction::Withdrawal { client, tx, amount }),
        2 => (client.clone(), tx.clone())
            .prop_map(|(client, tx)| Transaction::Dispute { client, tx }),
        1 => (client.clone(), tx.clone())
            .prop_map(|(client, tx)| Transaction::Resolve { client, tx }),
        1 => (client, tx).prop_map(|(client, tx)| Transaction::Chargeback { client, tx }),
    ]
}

fn snapshot(engine: &Engine) -> BTreeMap<ClientId, Account> {
    engine
        .accounts()
        .map(|(client, account)| (client, account.clone()))
        .collect()
}

fn sum_of_totals(accounts: &BTreeMap<ClientId, Account>) -> Amount {
    accounts.values().fold(Amount::ZERO, |sum, account| {
        sum.checked_add(account.total()).unwrap()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn balances_stay_consistent(transactions in prop::collection::vec(transaction(), 1..60)) {
        let mut engine = Engine::default();
        // What the money in the system should add up to, tracked from the
        // outside using only which transactions the engine accepted.
        let mut expected_total = Amount::ZERO;
        let mut accepted_deposits: HashMap<TxId, Amount> = HashMap::new();
        let mut used_ids: HashSet<TxId> = HashSet::new();

        for transaction in transactions {
            let before = snapshot(&engine);
            let result = engine.apply(transaction);
            let after = snapshot(&engine);

            match (result, transaction) {
                (Err(_), _) => {
                    // A rejected row changes nothing. Deposits and withdrawals
                    // may still create an empty account for their client.
                    for (client, account) in &after {
                        match before.get(client) {
                            Some(old) => prop_assert_eq!(old, account),
                            None => prop_assert_eq!(account, &Account::default()),
                        }
                    }
                }
                (Ok(()), Transaction::Deposit { tx, amount, .. }) => {
                    prop_assert!(used_ids.insert(tx), "tx {tx} accepted twice");
                    expected_total = expected_total.checked_add(amount).unwrap();
                    accepted_deposits.insert(tx, amount);
                }
                (Ok(()), Transaction::Withdrawal { tx, amount, .. }) => {
                    prop_assert!(used_ids.insert(tx), "tx {tx} accepted twice");
                    expected_total = expected_total.checked_sub(amount).unwrap();
                }
                (Ok(()), Transaction::Chargeback { tx, .. }) => {
                    expected_total = expected_total.checked_sub(accepted_deposits[&tx]).unwrap();
                }
                (Ok(()), Transaction::Dispute { .. } | Transaction::Resolve { .. }) => {}
            }

            for (client, account) in &after {
                prop_assert!(account.held() >= Amount::ZERO, "client {client}: negative held");
                prop_assert_eq!(
                    account.total(),
                    account.available().checked_add(account.held()).unwrap()
                );
                if let Some(old) = before.get(client).filter(|old| old.is_locked()) {
                    prop_assert!(account.is_locked(), "client {client} was unlocked");
                    prop_assert!(account.total() <= old.total(), "locked client {client} gained funds");
                }
            }
            prop_assert_eq!(sum_of_totals(&after), expected_total);
        }
    }

    #[test]
    fn remembering_only_disputed_deposits_changes_nothing(
        transactions in prop::collection::vec(transaction(), 1..60),
    ) {
        let mut disputed = TxIdSet::default();
        for transaction in &transactions {
            if let Transaction::Dispute { tx, .. } = transaction {
                disputed.insert(*tx);
            }
        }
        let mut every_deposit = Engine::default();
        let mut only_disputed = Engine::remembering_only(disputed);

        for transaction in transactions {
            // The error may differ (unknown tx instead of not under dispute),
            // but whether the row is accepted must not.
            prop_assert_eq!(
                every_deposit.apply(transaction).is_ok(),
                only_disputed.apply(transaction).is_ok(),
                "{:?}", transaction
            );
            prop_assert_eq!(snapshot(&every_deposit), snapshot(&only_disputed));
        }
    }
}
