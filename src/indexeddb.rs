//! Bounded clean-room IndexedDB 3.0 implementation for GhitaBrowser (Phase 22).
//! Provides transactional key-value & indexed storage with full rollback support on abort.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const MAX_DBS_PER_ORIGIN: usize = 64;
pub const MAX_STORES_PER_DB: usize = 256;
pub const MAX_INDEXES_PER_STORE: usize = 1024;
pub const MAX_RECORDS_PER_ORIGIN: usize = 100_000;
pub const MAX_RECORD_VALUE_BYTES: usize = 2 * 1024 * 1024; // 2 MB per record

/// W3C IndexedDB Key type with canonical comparison ordering:
/// Array > String > Date > Number > Binary.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IDBKey {
    Array(Vec<IDBKey>),
    String(String),
    Date(f64),
    Number(f64),
    Binary(Vec<u8>),
}

impl PartialOrd for IDBKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for IDBKey {}

impl Ord for IDBKey {
    fn cmp(&self, other: &Self) -> Ordering {
        fn type_rank(key: &IDBKey) -> u8 {
            match key {
                IDBKey::Array(_) => 5,
                IDBKey::String(_) => 4,
                IDBKey::Date(_) => 3,
                IDBKey::Number(_) => 2,
                IDBKey::Binary(_) => 1,
            }
        }

        let rank_a = type_rank(self);
        let rank_b = type_rank(other);
        if rank_a != rank_b {
            return rank_a.cmp(&rank_b);
        }

        match (self, other) {
            (IDBKey::Array(a), IDBKey::Array(b)) => {
                let min_len = a.len().min(b.len());
                for i in 0..min_len {
                    let ord = a[i].cmp(&b[i]);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                a.len().cmp(&b.len())
            }
            (IDBKey::String(a), IDBKey::String(b)) => a.cmp(b),
            (IDBKey::Date(a), IDBKey::Date(b)) => {
                if a.is_nan() && b.is_nan() {
                    Ordering::Equal
                } else if a.is_nan() {
                    Ordering::Less
                } else if b.is_nan() {
                    Ordering::Greater
                } else {
                    a.partial_cmp(b).unwrap_or(Ordering::Equal)
                }
            }
            (IDBKey::Number(a), IDBKey::Number(b)) => {
                if a.is_nan() && b.is_nan() {
                    Ordering::Equal
                } else if a.is_nan() {
                    Ordering::Less
                } else if b.is_nan() {
                    Ordering::Greater
                } else {
                    a.partial_cmp(b).unwrap_or(Ordering::Equal)
                }
            }
            (IDBKey::Binary(a), IDBKey::Binary(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IDBIndexConfig {
    pub name: String,
    pub key_path: String,
    pub unique: bool,
    pub multi_entry: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IDBRecord {
    pub key: IDBKey,
    pub value: String, // JSON payload
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IDBObjectStore {
    pub name: String,
    pub key_path: Option<String>,
    pub auto_increment: bool,
    pub auto_increment_counter: u64,
    pub records: Vec<IDBRecord>,
    pub indexes: HashMap<String, IDBIndexConfig>,
}

impl IDBObjectStore {
    pub fn new(name: impl Into<String>, key_path: Option<String>, auto_increment: bool) -> Self {
        Self {
            name: name.into(),
            key_path,
            auto_increment,
            auto_increment_counter: 1,
            records: Vec::new(),
            indexes: HashMap::new(),
        }
    }

    pub fn get(&self, key: &IDBKey) -> Option<&IDBRecord> {
        self.records.iter().find(|r| &r.key == key)
    }

    pub fn put(&mut self, key: Option<IDBKey>, value: String) -> Result<IDBKey, String> {
        if value.len() > MAX_RECORD_VALUE_BYTES {
            return Err("Record value exceeds 2 MB budget".to_string());
        }

        let key = match key {
            Some(k) => k,
            None if self.auto_increment => {
                let id = self.auto_increment_counter;
                self.auto_increment_counter += 1;
                IDBKey::Number(id as f64)
            }
            None => return Err("Key required when autoIncrement is false".to_string()),
        };

        if let Some(pos) = self.records.iter().position(|r| r.key == key) {
            self.records[pos].value = value;
        } else {
            self.records.push(IDBRecord {
                key: key.clone(),
                value,
            });
            self.records.sort_by(|a, b| a.key.cmp(&b.key));
        }

        Ok(key)
    }

    pub fn add(&mut self, key: Option<IDBKey>, value: String) -> Result<IDBKey, String> {
        let actual_key = match &key {
            Some(k) => k,
            None if self.auto_increment => &IDBKey::Number(self.auto_increment_counter as f64),
            None => return Err("Key required when autoIncrement is false".to_string()),
        };

        if self.get(actual_key).is_some() {
            return Err("Key already exists in object store".to_string());
        }

        self.put(key, value)
    }

    pub fn delete(&mut self, key: &IDBKey) -> bool {
        if let Some(pos) = self.records.iter().position(|r| &r.key == key) {
            self.records.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn get_by_index(&self, index_name: &str, index_key: &IDBKey) -> Option<&IDBRecord> {
        let config = self.indexes.get(index_name)?;
        for record in &self.records {
            if let Ok(extracted_key) = extract_key_from_json(&record.value, &config.key_path) {
                if &extracted_key == index_key {
                    return Some(record);
                }
            }
        }
        None
    }
}

fn extract_key_from_json(json_str: &str, key_path: &str) -> Result<IDBKey, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON: {e}"))?;
    let mut current = &parsed;
    for part in key_path.split('.') {
        current = current
            .get(part)
            .ok_or_else(|| format!("Key path '{key_path}' not found in JSON"))?;
    }
    match current {
        serde_json::Value::Number(n) => Ok(IDBKey::Number(
            n.as_f64().ok_or_else(|| "Invalid number".to_string())?,
        )),
        serde_json::Value::String(s) => Ok(IDBKey::String(s.clone())),
        _ => Err(format!("Unsupported key type at '{key_path}'")),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IDBDatabase {
    pub name: String,
    pub version: u64,
    pub object_stores: HashMap<String, IDBObjectStore>,
}

impl IDBDatabase {
    pub fn new(name: impl Into<String>, version: u64) -> Self {
        Self {
            name: name.into(),
            version,
            object_stores: HashMap::new(),
        }
    }

    pub fn create_object_store(
        &mut self,
        name: impl Into<String>,
        key_path: Option<String>,
        auto_increment: bool,
    ) -> Result<&mut IDBObjectStore, String> {
        let store_name = name.into();
        if self.object_stores.len() >= MAX_STORES_PER_DB {
            return Err("Max object stores per DB exceeded".to_string());
        }
        if self.object_stores.contains_key(&store_name) {
            return Err(format!("ObjectStore '{store_name}' already exists"));
        }
        let store = IDBObjectStore::new(store_name.clone(), key_path, auto_increment);
        self.object_stores.insert(store_name.clone(), store);
        Ok(self.object_stores.get_mut(&store_name).expect("inserted"))
    }

    pub fn delete_object_store(&mut self, name: &str) -> bool {
        self.object_stores.remove(name).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IDBTransactionMode {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

pub struct IDBTransaction {
    pub id: u64,
    pub mode: IDBTransactionMode,
    pub scope: Vec<String>,
    pub db_name: String,
    pub rollback_snapshot: Option<IDBDatabase>,
    pub aborted: bool,
    pub committed: bool,
}

#[derive(Debug, Default)]
pub struct IndexedDBEngine {
    pub origin: String,
    pub databases: HashMap<String, IDBDatabase>,
    next_tx_id: u64,
    storage_path: Option<PathBuf>,
}

impl IndexedDBEngine {
    pub fn new(origin: impl Into<String>, storage_path: Option<PathBuf>) -> Self {
        let origin = origin.into();
        let mut engine = Self {
            origin,
            databases: HashMap::new(),
            next_tx_id: 1,
            storage_path,
        };
        let _ = engine.load_from_disk();
        engine
    }

    pub fn open_db(
        &mut self,
        name: &str,
        version: Option<u64>,
    ) -> Result<(IDBTransaction, &mut IDBDatabase), String> {
        if self.databases.len() >= MAX_DBS_PER_ORIGIN && !self.databases.contains_key(name) {
            return Err("Max databases per origin budget exceeded".to_string());
        }

        let target_version = version.unwrap_or(1);
        let is_new_or_upgrade = match self.databases.get(name) {
            None => true,
            Some(db) => target_version > db.version,
        };

        if let Some(db) = self.databases.get(name) {
            if target_version < db.version {
                return Err("VersionRequestedBelowCurrent".to_string());
            }
        }

        if !self.databases.contains_key(name) {
            self.databases
                .insert(name.to_string(), IDBDatabase::new(name, target_version));
        }

        let db = self.databases.get_mut(name).expect("db exists");
        if is_new_or_upgrade {
            db.version = target_version;
        }

        let mode = if is_new_or_upgrade {
            IDBTransactionMode::VersionChange
        } else {
            IDBTransactionMode::ReadOnly
        };

        let tx_id = self.next_tx_id;
        self.next_tx_id += 1;

        let tx = IDBTransaction {
            id: tx_id,
            mode,
            scope: db.object_stores.keys().cloned().collect(),
            db_name: name.to_string(),
            rollback_snapshot: Some(db.clone()),
            aborted: false,
            committed: false,
        };

        Ok((tx, db))
    }

    pub fn commit_transaction(&mut self, mut tx: IDBTransaction) -> Result<(), String> {
        if tx.aborted {
            return Err("Transaction was aborted".to_string());
        }
        tx.committed = true;
        tx.rollback_snapshot = None;
        let _ = self.save_to_disk();
        Ok(())
    }

    pub fn abort_transaction(&mut self, mut tx: IDBTransaction) -> Result<(), String> {
        if tx.committed {
            return Err("Transaction was already committed".to_string());
        }
        tx.aborted = true;
        if let Some(snapshot) = tx.rollback_snapshot.take() {
            self.databases.insert(tx.db_name.clone(), snapshot);
        }
        Ok(())
    }

    pub fn delete_database(&mut self, name: &str) -> bool {
        let removed = self.databases.remove(name).is_some();
        if removed {
            let _ = self.save_to_disk();
        }
        removed
    }

    pub fn total_records(&self) -> usize {
        self.databases
            .values()
            .map(|db| db.object_stores.values().map(|s| s.count()).sum::<usize>())
            .sum()
    }

    /// Persist the current committed state for browser-host integrations.
    pub fn persist(&self) -> Result<(), String> {
        self.save_to_disk().map_err(|error| error.to_string())
    }

    fn save_to_disk(&self) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[derive(serde::Serialize)]
        struct Persisted<'a> {
            schema: u32,
            origin: &'a str,
            databases: &'a HashMap<String, IDBDatabase>,
        }
        let bytes = serde_json::to_vec(&Persisted {
            schema: 1,
            origin: &self.origin,
            databases: &self.databases,
        })
        .map_err(std::io::Error::other)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    fn load_from_disk(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        #[derive(serde::Deserialize)]
        struct Persisted {
            schema: u32,
            origin: String,
            databases: HashMap<String, IDBDatabase>,
        }
        let bytes = fs::read(path)?;
        let persisted: Persisted = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        if persisted.schema != 1 || persisted.origin != self.origin {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IndexedDB origin/schema mismatch",
            ));
        }
        self.databases = persisted.databases;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idb_key_w3c_ordering_is_correct() {
        let num_1 = IDBKey::Number(1.0);
        let num_2 = IDBKey::Number(2.0);
        let str_a = IDBKey::String("a".to_string());
        let str_b = IDBKey::String("b".to_string());
        let arr_1 = IDBKey::Array(vec![IDBKey::Number(1.0)]);

        assert!(num_1 < num_2);
        assert!(str_a < str_b);
        assert!(num_2 < str_a);
        assert!(str_b < arr_1);
    }

    #[test]
    fn idb_transaction_rollback_reverts_mutations() {
        let mut engine = IndexedDBEngine::new("https://example.com", None);
        let (tx, db) = engine.open_db("test_db", Some(1)).unwrap();
        db.create_object_store("users", None, true).unwrap();
        engine.commit_transaction(tx).unwrap();

        // Start a readwrite transaction and mutate
        let (tx2, db2) = engine.open_db("test_db", Some(1)).unwrap();
        let store = db2.object_stores.get_mut("users").unwrap();
        store.put(None, "{\"name\":\"Alice\"}".to_string()).unwrap();
        assert_eq!(store.count(), 1);

        // Abort transaction
        engine.abort_transaction(tx2).unwrap();

        // Verify state is reverted
        let db_after = engine.databases.get("test_db").unwrap();
        let store_after = db_after.object_stores.get("users").unwrap();
        assert_eq!(store_after.count(), 0);
    }
}
