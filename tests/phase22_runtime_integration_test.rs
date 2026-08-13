use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ghitabrowser::javascript::JsvValue;
use ghitabrowser::web_runtime::PageRuntime;

fn isolated_storage_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ghita-phase22-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn page_javascript_uses_persistent_indexeddb_cache_and_service_worker() {
    let storage_root = isolated_storage_root("persistent-runtime");
    {
        let mut page = PageRuntime::from_html_with_storage_dir(
            "<main>offline app</main>",
            Vec::new(),
            800,
            "https://app.example.test/index.html",
            Some(&storage_root),
        )
        .expect("page runtime");
        page.evaluate(
            r#"
                let db = indexedDB.open("application", 1);
                let records = db.createObjectStore("records", null, false);
                records.put({name: "persisted", count: 3}, "primary");
                let offline = caches.open("offline-v1");
                offline.put("/offline.txt", "available offline", 200);
                let registration = navigator.serviceWorker.register("/sw.js", {scope: "/"});
            "#,
        )
        .expect("platform script");

        assert_eq!(
            page.evaluate("records.get('primary').name")
                .expect("read record"),
            JsvValue::String("persisted".to_string())
        );
        assert_eq!(
            page.evaluate("offline.match('/offline.txt')")
                .expect("read cache"),
            JsvValue::String("available offline".to_string())
        );
        assert_eq!(
            page.evaluate("registration.scope")
                .expect("registration scope"),
            JsvValue::String("/".to_string())
        );
        assert!(page.report().platform_operations.len() >= 6);
    }

    {
        let mut restored = PageRuntime::from_html_with_storage_dir(
            "<main>restored app</main>",
            Vec::new(),
            800,
            "https://app.example.test/restore.html",
            Some(&storage_root),
        )
        .expect("restored runtime");
        restored
            .evaluate(
                r#"
                    let restoredDb = indexedDB.open("application", 1);
                    let restoredRecords = restoredDb.objectStore("records");
                    let restoredCache = caches.open("offline-v1");
                "#,
            )
            .expect("open restored stores");
        assert_eq!(
            restored
                .evaluate("restoredRecords.get('primary').count")
                .expect("restored record"),
            JsvValue::Number(3.0)
        );
        assert_eq!(
            restored
                .evaluate("restoredCache.match('/offline.txt')")
                .expect("restored cache"),
            JsvValue::String("available offline".to_string())
        );
    }

    std::fs::remove_dir_all(storage_root).expect("remove isolated test storage");
}

#[test]
fn page_javascript_messaging_is_deep_cloned_and_origin_partitioned() {
    let mut page = PageRuntime::from_html(
        "<main>messages</main>",
        Vec::new(),
        800,
        "https://messages.example.test/",
    )
    .expect("page runtime");

    page.evaluate(
        r#"
            let sender = BroadcastChannel("updates");
            let receiver = BroadcastChannel("updates");
            let original = {nested: {value: 7}};
            let copy = structuredClone(original);
            original.nested.value = 9;
            sender.postMessage(copy);
            let received = receiver.poll();
        "#,
    )
    .expect("messaging script");

    assert_eq!(
        page.evaluate("received.nested.value")
            .expect("received clone"),
        JsvValue::Number(7.0)
    );
    assert_eq!(
        page.evaluate("copy.nested.value")
            .expect("standalone clone"),
        JsvValue::Number(7.0)
    );
}
