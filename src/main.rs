use anyhow::{bail, Context, Result};
use csv_async::{AsyncDeserializer, AsyncReaderBuilder, AsyncSerializer, Trim};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use tokio::fs::File;
use tokio::io;
use tokio_stream::StreamExt;

async fn parse_transactions(input_file: &str) -> Result<HashMap<u16, Client>> {
    let mut reader = AsyncReaderBuilder::new()
        .trim(Trim::All)
        .create_deserializer(File::open(&input_file).await.with_context(|| {
            format!("Failed to read transaction input file from {}", &input_file)
        })?);
    // keeping transactions and clients in memory while streaming the file doesn't make sense here
    // This is where using sqlite would come in handy
    let mut clients: HashMap<u16, Client> = HashMap::new();
    let mut transactions: HashMap<u32, TransactionRecord> = HashMap::new();
    // I'm assuming that the number of disputed transactions is quite low
    // so this allocation shouldn't be an issue
    let mut disputed: HashMap<u32, TransactionRecord> = HashMap::new();
    let mut records = AsyncDeserializer::deserialize::<TransactionRecord>(&mut reader);
    while let Some(record) = records.next().await {
        // TODO: this is probably best expressed as a combinator
        if let Ok(record) = record {
            process_transaction(&mut clients, &mut transactions, &mut disputed, record);
        } else {
            eprintln!(
                "failed to parse into record, ignoring transaction, received input {:?}",
                record
            );
        }
    }
    Ok(clients)
}

fn process_transaction(
    clients: &mut HashMap<u16, Client>,
    transactions: &mut HashMap<u32, TransactionRecord>,
    disputed: &mut HashMap<u32, TransactionRecord>,
    record: TransactionRecord,
) {
    let client = clients
        .entry(record.client_id)
        .or_insert_with(|| Client::new(record.client_id));
    match &record.transaction_type {
        &TransactionType::Deposit => {
            let amount = &record.amount.unwrap().clone();
            // in reality this would probably be a db transaction where both operations have to succeed together
            transactions.insert(record.transaction_id, record);
            (*client).deposit(amount)
        }
        &TransactionType::Withdrawal => {
            let amount = &record.amount.unwrap().clone();
            transactions.insert(record.transaction_id, record);
            (*client).withdraw(amount)
        }
        &TransactionType::Dispute => {
            if let Some(transaction) = transactions.get(&record.transaction_id) {
                if transaction.client_id == record.client_id {
                    (*client).dispute(&transaction);
                    disputed.insert(transaction.transaction_id, (*transaction).clone());
                } else {
                    eprintln!(
                        "dispute on transaction {} contains a different client_id",
                        &record.transaction_id
                    )
                }
            }
        }
        &TransactionType::Resolve => {
            if let Some(transaction) = disputed.remove(&record.transaction_id) {
                if transaction.client_id == record.client_id {
                    (*client).resolve(&transaction);
                } else {
                    eprintln!(
                        "resolve on transaction {} contains a different client_id",
                        &record.transaction_id
                    )
                }
            }
        }
        &TransactionType::Chargeback => {
            if let Some(transaction) = disputed.remove(&record.transaction_id) {
                if transaction.client_id == record.client_id {
                    (*client).chargeback();
                } else {
                    eprintln!(
                        "chargeback on transaction {} contains a different client_id",
                        &record.transaction_id
                    )
                }
            }
        }
    }
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
    let accounts = parse_transactions(&args[1]).await?;
    let mut writer = AsyncSerializer::from_writer(io::stdout());
    for (_id, client) in &accounts {
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

#[derive(Debug, Serialize)]
struct Client {
    id: u16,
    available: Decimal,
    held: Decimal,
    total: Decimal,
    locked: bool,
}

impl Client {
    fn new(id: u16) -> Self {
        Self {
            id,
            available: Decimal::ZERO,
            held: Decimal::ZERO,
            total: Decimal::ZERO,
            locked: false,
        }
    }

    fn deposit(&mut self, amount: &Decimal) {
        self.available += amount;
        self.total += amount;
    }

    fn withdraw(&mut self, amount: &Decimal) {
        if &self.available >= amount {
            self.available -= amount;
            self.total -= amount;
        }
    }

    fn dispute(&mut self, transaction: &TransactionRecord) {
        match transaction.transaction_type {
            TransactionType::Deposit => {
                let amount = transaction.amount.unwrap();
                if self.available < amount {
                    // not sure what to do here, the person has already taken the money out of the account
                    // the most logical seem to lock the account
                    eprintln!("Client {} already took the money out", self.id);
                    eprintln!("this shouldn't be possible")
                } else {
                    self.available -= amount;
                    self.held += amount;
                }
            }
            TransactionType::Withdrawal => {
                let amount = transaction.amount.unwrap();
                self.held += amount;
                self.total += amount;
            }
            _ => {
                eprintln!(
                    "disputing type {:?} hasn't been implemented",
                    transaction.transaction_type
                );
            }
        }
    }

    fn resolve(&mut self, transaction: &TransactionRecord) {
        match transaction.transaction_type {
            TransactionType::Deposit => {
                let amount = transaction.amount.unwrap();
                if self.held < amount {
                    // not sure what to do here, the person has already taken the money out of the account
                    // the most logical seem to lock the account
                    // this shouldn't be possible though
                    eprintln!("Client {} already took the money out", self.id);
                    eprintln!("this shouldn't be possible")
                } else {
                    self.held -= amount;
                    self.available += amount;
                }
            }
            TransactionType::Withdrawal => {
                let amount = transaction.amount.unwrap();
                if self.held < amount {
                    // not sure what to do here, the person has already taken the money out of the account
                    // the most logical seem to lock the account
                    // this shouldn't be possible though
                    eprintln!("Client {} already took the money out", self.id);
                    eprintln!("this shouldn't be possible")
                } else {
                    self.held -= amount;
                    self.total -= amount;
                }
            }
            _ => {
                eprintln!(
                    "resolving type {:?} hasn't been implemented",
                    transaction.transaction_type
                );
            }
        }
    }
    fn chargeback(&mut self) {
        self.locked = true;
    }
}
