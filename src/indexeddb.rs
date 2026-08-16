//! Bounded clean-room IndexedDB 3.0 implementation for GhitaBrowser (Phase 22).
//! Provides transactional key-value & indexed storage with full rollback support on abort.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;

pub const MAX_DBS_PER_ORIGIN: usize = 64;
pub const MAX_STORES_PER_DB: usize = 256;
pub const MAX_INDEXES_PER_STORE: usize = 1024;
pub const MAX_RECORDS_PER_ORIGIN: usize = 100_000;
pub const MAX_RECORD_VALUE_BYTES: usize = 2 * 1024 * 1024; // 2 MB per record
pub const MAX_CURSOR_RESULTS: usize = 10_000;
pub const MAX_INDEX_SCAN_MATCHES: usize = 100_000;
pub const MAX_KEY_BYTES: usize = 64 * 1024;
pub const MAX_OBJECT_STORE_VALUE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ORIGIN_VALUE_BYTES: usize = 256 * 1024 * 1024;

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

/// Inclusive/exclusive bounds used by deterministic cursor and index scans.
/// A missing bound is unbounded in that direction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IDBKeyRange {
    pub lower: Option<IDBKey>,
    pub upper: Option<IDBKey>,
    pub lower_open: bool,
    pub upper_open: bool,
}

impl IDBKeyRange {
    pub fn only(key: IDBKey) -> Self {
        Self {
            lower: Some(key.clone()),
            upper: Some(key),
            lower_open: false,
            upper_open: false,
        }
    }

    pub fn contains(&self, key: &IDBKey) -> bool {
        let lower_ok = self.lower.as_ref().is_none_or(|lower| {
            if self.lower_open {
                key > lower
            } else {
                key >= lower
            }
        });
        let upper_ok = self.upper.as_ref().is_none_or(|upper| {
            if self.upper_open {
                key < upper
            } else {
                key <= upper
            }
        });
        lower_ok && upper_ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IDBCursorDirection {
    Next,
    NextUnique,
    Prev,
    PrevUnique,
}

/// Snapshot cursor with deterministic ordering. It intentionally carries
/// cloned records so a later mutation cannot invalidate a page-visible cursor
/// or leak an internal borrow across an asynchronous JavaScript turn.
#[derive(Debug, Clone)]
pub struct IDBCursor {
    records: Vec<IDBRecord>,
    position: usize,
}

impl IDBCursor {
    pub fn from_records(records: Vec<IDBRecord>) -> Self {
        Self {
            records,
            position: 0,
        }
    }

    pub fn current(&self) -> Option<&IDBRecord> {
        self.records.get(self.position)
    }

    pub fn advance(&mut self, count: usize) -> Option<&IDBRecord> {
        self.position = self.position.saturating_add(count.max(1));
        self.current()
    }

    pub fn remaining(&self) -> usize {
        self.records.len().saturating_sub(self.position)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IDBRecord {
    pub key: IDBKey,
    /// Shared immutable JSON payload. Cursor and transaction snapshots clone
    /// only this pointer instead of duplicating every record body in RAM.
    pub value: Arc<str>,
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

        validate_key_budget(&key, 0)?;
        let previous_bytes = self
            .records
            .iter()
            .find(|record| record.key == key)
            .map(|record| record.value.len())
            .unwrap_or(0);
        let retained_bytes = self
            .records
            .iter()
            .map(|record| record.value.len())
            .sum::<usize>();
        let projected_bytes = retained_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(value.len());
        if projected_bytes > MAX_OBJECT_STORE_VALUE_BYTES {
            return Err("Object store value-byte budget exceeded".to_string());
        }
        if previous_bytes == 0 && self.records.len() >= MAX_RECORDS_PER_ORIGIN {
            return Err("Object store record budget exceeded".to_string());
        }

        self.validate_indexes(&key, &value)?;
        if let Some(pos) = self.records.iter().position(|r| r.key == key) {
            self.records[pos].value = Arc::from(value);
        } else {
            self.records.push(IDBRecord {
                key: key.clone(),
                value: Arc::from(value),
            });
            self.records.sort_by(|a, b| a.key.cmp(&b.key));
        }

        Ok(key)
    }

    pub fn add(&mut self, key: Option<IDBKey>, value: String) -> Result<IDBKey, String> {
        let actual_key = match &key {
            Some(k) => k.clone(),
            None if self.auto_increment => IDBKey::Number(self.auto_increment_counter as f64),
            None => return Err("Key required when autoIncrement is false".to_string()),
        };

        if self.get(&actual_key).is_some() {
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
        self.records.iter().find(|record| {
            extract_keys_from_json(&record.value, config)
                .ok()
                .is_some_and(|keys| keys.iter().any(|key| key == index_key))
        })
    }

    pub fn create_index(&mut self, config: IDBIndexConfig) -> Result<(), String> {
        if config.name.is_empty() || config.name.len() > 256 || config.key_path.is_empty() {
            return Err("Index name and key path are required".to_string());
        }
        if self.indexes.len() >= MAX_INDEXES_PER_STORE && !self.indexes.contains_key(&config.name) {
            return Err("Max indexes per object store exceeded".to_string());
        }
        if self.indexes.contains_key(&config.name) {
            return Err(format!("Index '{}' already exists", config.name));
        }
        if config.unique {
            let mut seen = BTreeSet::new();
            for record in &self.records {
                for key in extract_keys_from_json(&record.value, &config)? {
                    if !seen.insert(key) {
                        return Err(
                            "ConstraintError: unique index contains duplicate keys".to_string()
                        );
                    }
                }
            }
        }
        self.indexes.insert(config.name.clone(), config);
        Ok(())
    }

    pub fn delete_index(&mut self, name: &str) -> bool {
        self.indexes.remove(name).is_some()
    }

    pub fn get_all_by_index(
        &self,
        index_name: &str,
        range: Option<&IDBKeyRange>,
        limit: usize,
    ) -> Result<Vec<IDBRecord>, String> {
        let config = self
            .indexes
            .get(index_name)
            .ok_or_else(|| format!("Index '{index_name}' does not exist"))?;
        let mut results: Vec<(IDBKey, &IDBRecord)> = Vec::new();
        for record in &self.records {
            for key in extract_keys_from_json(&record.value, config)? {
                if range.is_none_or(|range| range.contains(&key)) {
                    if results.len() >= MAX_INDEX_SCAN_MATCHES {
                        return Err(
                            "QuotaExceededError: IndexedDB index scan exceeds budget".to_string()
                        );
                    }
                    results.push((key, record));
                }
            }
        }
        results.sort_by(|(left_key, left_record), (right_key, right_record)| {
            left_key
                .cmp(right_key)
                .then_with(|| left_record.key.cmp(&right_record.key))
        });
        let mut unique_primary = Vec::new();
        let mut seen_primary = BTreeSet::new();
        for (_, record) in results {
            if seen_primary.insert(record.key.clone()) {
                unique_primary.push(record.clone());
            }
            if unique_primary.len() >= limit.min(MAX_CURSOR_RESULTS) {
                break;
            }
        }
        Ok(unique_primary)
    }

    pub fn open_cursor(
        &self,
        range: Option<&IDBKeyRange>,
        direction: IDBCursorDirection,
    ) -> IDBCursor {
        let mut records: Vec<IDBRecord> = if matches!(
            direction,
            IDBCursorDirection::Prev | IDBCursorDirection::PrevUnique
        ) {
            self.records
                .iter()
                .rev()
                .filter(|record| range.is_none_or(|range| range.contains(&record.key)))
                .take(MAX_CURSOR_RESULTS)
                .cloned()
                .collect()
        } else {
            self.records
                .iter()
                .filter(|record| range.is_none_or(|range| range.contains(&record.key)))
                .take(MAX_CURSOR_RESULTS)
                .cloned()
                .collect()
        };
        if matches!(
            direction,
            IDBCursorDirection::NextUnique | IDBCursorDirection::PrevUnique
        ) {
            records.dedup_by(|left, right| left.key == right.key);
        }
        IDBCursor::from_records(records)
    }

    fn validate_indexes(&self, primary_key: &IDBKey, value: &str) -> Result<(), String> {
        for config in self.indexes.values().filter(|config| config.unique) {
            let proposed_keys = extract_keys_from_json(value, config)?;
            for record in &self.records {
                if &record.key == primary_key {
                    continue;
                }
                let existing_keys = extract_keys_from_json(&record.value, config)?;
                if proposed_keys
                    .iter()
                    .any(|proposed| existing_keys.iter().any(|existing| existing == proposed))
                {
                    return Err(format!(
                        "ConstraintError: unique index '{}' already contains this key",
                        config.name
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_key_budget(key: &IDBKey, depth: usize) -> Result<usize, String> {
    if depth > 32 {
        return Err("IndexedDB key nesting budget exceeded".to_string());
    }
    let bytes = match key {
        IDBKey::Array(values) => {
            if values.len() > 1_024 {
                return Err("IndexedDB array key element budget exceeded".to_string());
            }
            let mut bytes = 0usize;
            for value in values {
                bytes = bytes.saturating_add(validate_key_budget(value, depth + 1)?);
                if bytes > MAX_KEY_BYTES {
                    break;
                }
            }
            bytes
        }
        IDBKey::String(value) => value.len(),
        IDBKey::Binary(value) => value.len(),
        IDBKey::Date(_) | IDBKey::Number(_) => std::mem::size_of::<f64>(),
    };
    if bytes > MAX_KEY_BYTES {
        return Err("IndexedDB key byte budget exceeded".to_string());
    }
    Ok(bytes)
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
    json_value_to_key(current).map_err(|_| format!("Unsupported key type at '{key_path}'"))
}

fn json_value_to_key(value: &serde_json::Value) -> Result<IDBKey, String> {
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .filter(|number| number.is_finite())
            .map(IDBKey::Number)
            .ok_or_else(|| "Invalid number key".to_string()),
        serde_json::Value::String(value) if value.len() <= 64 * 1024 => {
            Ok(IDBKey::String(value.clone()))
        }
        serde_json::Value::Array(values) if values.len() <= 1_024 => values
            .iter()
            .map(json_value_to_key)
            .collect::<Result<Vec<_>, _>>()
            .map(IDBKey::Array),
        _ => Err("Unsupported IndexedDB key value".to_string()),
    }
}

fn extract_keys_from_json(json_str: &str, config: &IDBIndexConfig) -> Result<Vec<IDBKey>, String> {
    let key = extract_key_from_json(json_str, &config.key_path)?;
    if !config.multi_entry {
        return Ok(vec![key]);
    }
    match key {
        IDBKey::Array(values) => Ok(values),
        value => Ok(vec![value]),
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
        let retained_bytes = self
            .databases
            .values()
            .flat_map(|database| database.object_stores.values())
            .flat_map(|store| store.records.iter())
            .map(|record| record.value.len())
            .sum::<usize>();
        if self.total_records() > MAX_RECORDS_PER_ORIGIN || retained_bytes > MAX_ORIGIN_VALUE_BYTES
        {
            if let Some(snapshot) = tx.rollback_snapshot.take() {
                self.databases.insert(tx.db_name.clone(), snapshot);
            }
            return Err("IndexedDB origin storage budget exceeded".to_string());
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
        let temporary = path.with_extension("tmp");
        let file = fs::File::create(&temporary)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(
            &mut writer,
            &Persisted {
                schema: 1,
                origin: &self.origin,
                databases: &self.databases,
            },
        )
        .map_err(std::io::Error::other)?;
        writer.flush()?;
        drop(writer);
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
        if fs::metadata(path)?.len()
            > (MAX_ORIGIN_VALUE_BYTES as u64).saturating_add(16 * 1024 * 1024)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IndexedDB persistence file exceeds budget",
            ));
        }
        let file = fs::File::open(path)?;
        let persisted: Persisted =
            serde_json::from_reader(BufReader::new(file)).map_err(std::io::Error::other)?;
        if persisted.schema != 1 || persisted.origin != self.origin {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IndexedDB origin/schema mismatch",
            ));
        }
        let records = persisted
            .databases
            .values()
            .flat_map(|database| database.object_stores.values())
            .flat_map(|store| store.records.iter());
        let mut record_count = 0usize;
        let mut value_bytes = 0usize;
        for record in records {
            validate_key_budget(&record.key, 0)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            record_count = record_count.saturating_add(1);
            value_bytes = value_bytes.saturating_add(record.value.len());
        }
        if record_count > MAX_RECORDS_PER_ORIGIN || value_bytes > MAX_ORIGIN_VALUE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IndexedDB persisted data exceeds origin budget",
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

    #[test]
    fn previous_cursor_keeps_the_highest_bounded_records() {
        let mut store = IDBObjectStore::new("bounded", None, false);
        store.records = (0..(MAX_CURSOR_RESULTS + 5))
            .map(|index| IDBRecord {
                key: IDBKey::Number(index as f64),
                value: Arc::from("{}"),
            })
            .collect();

        let cursor = store.open_cursor(None, IDBCursorDirection::Prev);
        assert_eq!(
            cursor.current().map(|record| record.key.clone()),
            Some(IDBKey::Number((MAX_CURSOR_RESULTS + 4) as f64))
        );
        assert_eq!(cursor.remaining(), MAX_CURSOR_RESULTS);
    }
}
