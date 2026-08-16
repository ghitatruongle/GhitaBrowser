use ghitabrowser::indexeddb::{IDBKey, IDBTransactionMode, IndexedDBEngine};

#[test]
fn idb_open_create_store_put_get_delete() {
    let mut engine = IndexedDBEngine::new("https://app.example.com", None);

    // Open DB version 1 -> creates versionchange transaction
    let (tx, db) = engine.open_db("app_db", Some(1)).expect("open_db");
    assert_eq!(tx.mode, IDBTransactionMode::VersionChange);

    // Create object store
    let store = db
        .create_object_store("users", Some("id".to_string()), true)
        .expect("create_object_store");

    // Put records
    let k1 = store
        .put(None, "{\"id\":1,\"name\":\"Alice\"}".to_string())
        .expect("put 1");
    let k2 = store
        .put(None, "{\"id\":2,\"name\":\"Bob\"}".to_string())
        .expect("put 2");

    assert_eq!(k1, IDBKey::Number(1.0));
    assert_eq!(k2, IDBKey::Number(2.0));
    assert_eq!(store.count(), 2);

    // Commit transaction
    engine.commit_transaction(tx).expect("commit");

    // Re-query database
    let db_ref = engine.databases.get("app_db").expect("db exists");
    let store_ref = db_ref.object_stores.get("users").expect("store exists");

    let rec = store_ref.get(&IDBKey::Number(1.0)).expect("rec 1");
    assert!(rec.value.contains("Alice"));

    // Delete record
    let (tx2, db2) = engine.open_db("app_db", Some(1)).expect("open_db 2");
    let store_mut = db2.object_stores.get_mut("users").expect("store mut");
    assert!(store_mut.delete(&IDBKey::Number(1.0)));
    assert_eq!(store_mut.count(), 1);
    engine.commit_transaction(tx2).expect("commit 2");
}

#[test]
fn idb_transaction_abort_reverts_uncommitted_mutations() {
    let mut engine = IndexedDBEngine::new("https://app.example.com", None);

    // Initialize DB & Store
    let (tx, db) = engine.open_db("finance_db", Some(1)).expect("open");
    db.create_object_store("accounts", None, false)
        .expect("create store");
    engine.commit_transaction(tx).expect("commit init");

    // Mutate in transaction 2
    let (tx2, db2) = engine.open_db("finance_db", Some(1)).expect("open tx2");
    let store = db2.object_stores.get_mut("accounts").expect("get store");
    store
        .put(
            Some(IDBKey::String("acc1".to_string())),
            "{\"balance\":1000}".to_string(),
        )
        .expect("put acc1");
    assert_eq!(store.count(), 1);

    // Abort transaction 2
    engine.abort_transaction(tx2).expect("abort tx2");

    // Verify database state is restored
    let db_final = engine.databases.get("finance_db").expect("get db");
    let store_final = db_final.object_stores.get("accounts").expect("get store");
    assert_eq!(store_final.count(), 0);
}

#[test]
fn idb_w3c_key_comparison_order() {
    let binary_key = IDBKey::Binary(vec![0x01, 0x02]);
    let number_key = IDBKey::Number(100.0);
    let date_key = IDBKey::Date(1600000000.0);
    let string_key = IDBKey::String("hello".to_string());
    let array_key = IDBKey::Array(vec![IDBKey::Number(1.0)]);

    // Standard W3C order: Binary < Number < Date < String < Array
    assert!(binary_key < number_key);
    assert!(number_key < date_key);
    assert!(date_key < string_key);
    assert!(string_key < array_key);
}

#[test]
fn idb_disk_persistence_round_trip() {
    let temp_dir = std::env::temp_dir().join("ghitabrowser_phase22_idb_test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let db_path = temp_dir.join("idb_store.txt");

    {
        let mut engine = IndexedDBEngine::new("https://app.example.com", Some(db_path.clone()));
        let (tx, db) = engine.open_db("persisted_db", Some(2)).expect("open");
        let store = db
            .create_object_store("cache", None, true)
            .expect("create store");
        store
            .put(None, "{\"key\":\"val1\"}".to_string())
            .expect("put");
        engine.commit_transaction(tx).expect("commit");
    }

    // Reopen from disk
    {
        let engine2 = IndexedDBEngine::new("https://app.example.com", Some(db_path));
        assert!(engine2.databases.contains_key("persisted_db"));
        let db = engine2.databases.get("persisted_db").unwrap();
        assert_eq!(db.version, 2);
        let store = db.object_stores.get("cache").expect("persisted store");
        assert_eq!(store.count(), 1);
        assert!(store
            .get(&IDBKey::Number(1.0))
            .expect("persisted record")
            .value
            .contains("val1"));
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}
