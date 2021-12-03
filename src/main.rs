use crate::RecordedTransaction::*;
use crate::TransactionType::*;
use anyhow::{bail, Context, Result};
use csv_async::{AsyncDeserializer, AsyncReaderBuilder, AsyncSerializer, Trim};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use tokio::fs::File;
use tokio::io;
use tokio_stream::StreamExt;

#[derive(Debug)]
struct Account {
    available: Decimal,
    held: Decimal,
    transactions: BTreeMap<u32, RecordedTransaction>,
    locked: bool,
}

impl Default for Account {
    fn default() -> Self {
        Account {
            available: Decimal::ZERO,
            held: Decimal::ZERO,
            transactions: BTreeMap::new(),
            locked: false,
        }
    }
}

impl Account {
    fn for_export(&self, id: u16) -> Client {
        Client {
            id,
            available: self.available,
            held: self.held,
            total: self.available + self.held,
            locked: self.locked,
        }
    }

    fn deposit(&mut self, (transaction_id, amount): (u32, Decimal)) {
        // in reality this would probably be a db transaction where both operations have to succeed together
        self.available += amount;
        self.transactions.insert(
            transaction_id,
            Validated(Transaction::Deposit(transaction_id, amount)),
        );
    }

    fn withdraw(&mut self, (transaction_id, amount): (u32, Decimal)) {
        let transaction = Transaction::Withdrawal(transaction_id, amount);
        if self.locked || self.available < amount {
            self.transactions
                .insert(transaction_id, Rejected(transaction));
        } else {
            self.available -= amount;
            self.transactions
                .insert(transaction_id, Validated(transaction));
        }
    }

    fn dispute(&mut self, transaction_id: u32) {
        if let Some(transaction) = self.transactions.get_mut(&transaction_id) {
            match transaction.dispute() {
                Some(Dispute::Deposit { amount }) => {
                    self.available -= amount;
                    self.held += amount;
                }
                Some(Dispute::Withdrawal { amount }) => {
                    self.held += amount;
                }
                _ => (),
            }
        }
    }

    fn resolve(&mut self, transaction_id: u32) {
        if let Some(transaction) = self.transactions.get_mut(&transaction_id) {
            match transaction.resolve() {
                Some(Resolve::Deposit { amount }) => {
                    self.available += amount;
                    self.held -= amount;
                }
                Some(Resolve::Withdrawal { amount }) => {
                    self.held -= amount;
                }
                _ => (),
            }
        }
    }

    fn chargeback(&mut self, transaction_id: u32) {
        if let Some(transaction) = self.transactions.get_mut(&transaction_id) {
            match transaction.chargeback() {
                Some(_) => {
                    self.locked = true;
                }
                _ => (),
            }
        }
    }

    fn process(&mut self, record: TransactionRecord) {
        match record {
            TransactionRecord {
                transaction_type: Deposit,
                amount,
                transaction_id,
                ..
            } => {
                self.deposit((transaction_id, amount.unwrap()));
            }
            TransactionRecord {
                transaction_type: Withdrawal,
                amount,
                transaction_id,
                ..
            } => {
                self.withdraw((transaction_id, amount.unwrap()));
            }
            TransactionRecord {
                transaction_type: Dispute,
                transaction_id,
                ..
            } => self.dispute(transaction_id),
            TransactionRecord {
                transaction_type: Resolve,
                transaction_id,
                ..
            } => self.resolve(transaction_id),
            TransactionRecord {
                transaction_type: Chargeback,
                transaction_id,
                ..
            } => self.chargeback(transaction_id),
        }
    }
}

#[derive(Clone, Debug)]
enum RecordedTransaction {
    Validated(Transaction),
    Disputed(Transaction),
    Resolved(Transaction),
    ChargedBack(Transaction),
    // TODO add a reason enum for the rejection
    Rejected(Transaction),
}

#[derive(Clone, Debug)]
enum Transaction {
    // transaction id, amount
    Deposit(u32, Decimal),
    Withdrawal(u32, Decimal),
}

enum Dispute {
    Deposit { amount: Decimal },
    Withdrawal { amount: Decimal },
}

enum Resolve {
    Deposit { amount: Decimal },
    Withdrawal { amount: Decimal },
}

enum ChargeBack {
    //TODO: do something with the amount ?
    Deposit { amount: Decimal },
    Withdrawal { amount: Decimal },
}

impl RecordedTransaction {
    fn dispute(&mut self) -> Option<Dispute> {
        match *self {
            Validated(Transaction::Deposit(transaction_id, amount)) => {
                *self = Disputed(Transaction::Deposit(transaction_id, amount));
                Some(Dispute::Deposit { amount })
            }
            Validated(Transaction::Withdrawal(transaction_id, amount)) => {
                *self = Disputed(Transaction::Withdrawal(transaction_id, amount));
                Some(Dispute::Withdrawal { amount })
            }
            Disputed(_) => None,
            // can you dispute a resolved transaction ? I'm going to say no
            Resolved(_) => None,
            // can you dispute a chargeback transaction ? I'm going to say no
            ChargedBack(_) => None,
            // disputed a rejected transaction should not do anything
            Rejected(_) => None,
        }
    }

    fn resolve(&mut self) -> Option<Resolve> {
        match *self {
            Validated(_) => None,
            Disputed(Transaction::Deposit(transaction_id, amount)) => {
                *self = Resolved(Transaction::Deposit(transaction_id, amount));
                Some(Resolve::Deposit { amount })
            }
            Disputed(Transaction::Withdrawal(transaction_id, amount)) => {
                *self = Resolved(Transaction::Withdrawal(transaction_id, amount));
                Some(Resolve::Withdrawal { amount })
            }
            // can you resolve a resolved transaction ? I'm going to say no
            Resolved(_) => None,
            // can you resolve a chargeback transaction ? I'm going to say no
            ChargedBack(_) => None,
            // resolving a rejected transaction should not do anything
            Rejected(_) => None,
        }
    }

    fn chargeback(&mut self) -> Option<ChargeBack> {
        match *self {
            Validated(_) => None,
            Disputed(Transaction::Deposit(transaction_id, amount)) => {
                *self = ChargedBack(Transaction::Deposit(transaction_id, amount));
                Some(ChargeBack::Deposit { amount })
            }
            Disputed(Transaction::Withdrawal(transaction_id, amount)) => {
                *self = ChargedBack(Transaction::Withdrawal(transaction_id, amount));
                Some(ChargeBack::Withdrawal { amount })
            }
            // can you chargeback a resolved transaction ? I'm going to say no
            Resolved(_) => None,
            // can you chargeback a chargeback transaction ? I'm going to say no
            ChargedBack(_) => None,
            // chargeback a rejected transaction should not do anything
            Rejected(_) => None,
        }
    }
}

#[derive(Default)]
struct Bank {
    // TODO add clock to ensure transaction order
    // next_clock: u64,
    accounts: BTreeMap<u16, Account>,
}

impl Bank {
    fn process_transaction(&mut self, record: TransactionRecord) {
        let account = self
            .accounts
            .entry(record.client_id)
            .or_insert_with(|| Account::default());
        // let next_clock = self.next_clock;
        // self.next_clock += 1;
        account.process(record);
    }
}

async fn parse_transactions(input_file: &str) -> Result<Bank> {
    let mut reader = AsyncReaderBuilder::new()
        .trim(Trim::All)
        .create_deserializer(File::open(&input_file).await.with_context(|| {
            format!("Failed to read transaction input file from {}", &input_file)
        })?);
    let mut bank = Bank::default();
    let mut records = AsyncDeserializer::deserialize::<TransactionRecord>(&mut reader);
    while let Some(record) = records.next().await {
        // TODO: this is probably best expressed as a combinator
        if let Ok(record) = record {
            bank.process_transaction(record);
        } else {
            eprintln!(
                "failed to parse into record, ignoring transaction, received input {:?}",
                record
            );
        }
    }
    Ok(bank)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        bail!(
            "This program supports exactly one    
         => (*client) argument passed, it should be the filename. I've receieved {:?}",
            args
        );
    }
    let bank = parse_transactions(&args[1]).await?;
    let mut writer = AsyncSerializer::from_writer(io::stdout());
    for (id, account) in &bank.accounts {
        let client = account.for_export(*id);
        writer.serialize(client).await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, Clone)]
struct TransactionRecord {
    #[serde(alias = "type")]
    transaction_type: TransactionType,
    #[serde(alias = "client")]
    client_id: u16,
    #[serde(alias = "tx")]
    transaction_id: u32,
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
enum TransactionType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

#[derive(Debug, Serialize, PartialEq)]
struct Client {
    id: u16,
    available: Decimal,
    held: Decimal,
    total: Decimal,
    locked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::FromStr;

    #[tokio::test]
    async fn example_input() {
        let parsed = parse_transactions("./data/example_input.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::from_str("1.5").unwrap(),
                held: Decimal::ZERO,
                total: Decimal::from_str("1.5").unwrap(),
                locked: false,
            },
        );
        let parsed_client_2 = parsed.accounts.get(&2).unwrap().for_export(2);
        assert_eq!(
            parsed_client_2,
            Client {
                id: 2,
                available: Decimal::from_str("2").unwrap(),
                held: Decimal::ZERO,
                total: Decimal::from_str("2").unwrap(),
                locked: false,
            },
        );
    }

    #[tokio::test]
    async fn dispute_deposit_with_resolution() {
        let parsed = parse_transactions("./data/dispute_deposit_with_resolution.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::from_str("1").unwrap(),
                held: Decimal::ZERO,
                total: Decimal::from_str("1").unwrap(),
                locked: false,
            },
        );
    }

    #[tokio::test]
    async fn dispute_deposit_with_chargeback() {
        let parsed = parse_transactions("./data/dispute_deposit_with_chargeback.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::ZERO,
                held: Decimal::from_str("1").unwrap(),
                total: Decimal::from_str("1").unwrap(),
                locked: true,
            },
        );
    }

    #[tokio::test]
    async fn dispute_withdrawal_with_resolution() {
        let parsed = parse_transactions("./data/dispute_withdrawal_with_resolution.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::ZERO,
                held: Decimal::ZERO,
                total: Decimal::ZERO,
                locked: false,
            },
        );
    }

    #[tokio::test]
    async fn thief() {
        let parsed = parse_transactions("./data/thief.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::from_str("-1").unwrap(),
                held: Decimal::from_str("1").unwrap(),
                total: Decimal::ZERO,
                locked: true,
            },
        );
    }

    #[tokio::test]
    async fn locked_account_should_be_able_to_deposit() {
        let parsed = parse_transactions("./data/locked_account_deposit.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::ZERO,
                held: Decimal::from_str("1").unwrap(),
                total: Decimal::from_str("1").unwrap(),
                locked: true,
            },
        );
    }

    #[tokio::test]
    async fn locked_account_cant_withdraw() {
        let parsed = parse_transactions("./data/locked_account_withdrawal.csv")
            .await
            .expect("failed parsing example input");
        let parsed_client_1 = parsed.accounts.get(&1).unwrap().for_export(1);
        assert_eq!(
            parsed_client_1,
            Client {
                id: 1,
                available: Decimal::from_str("1").unwrap(),
                held: Decimal::from_str("1").unwrap(),
                total: Decimal::from_str("2").unwrap(),
                locked: true,
            },
        );
    }
}
