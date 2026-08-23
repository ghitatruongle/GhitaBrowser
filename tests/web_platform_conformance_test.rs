// Web Platform Support Matrix conformance (track 9). Every entry in
// tests/fixtures/web-platform-support-matrix.json is driven through the real
// product path (PageRuntime::evaluate):
//   - implemented   -> probe must evaluate and match the expected boolean
//   - unsupported   -> probe must FAIL CLOSED (window.* throws, navigator.*
//                      stays undefined) instead of silently degrading.
// This test is the enforcement half of the matrix; the fixture is the
// documentation half, so editing one without the other breaks CI.

use ghitabrowser::javascript::JsvValue;
use ghitabrowser::web_runtime::PageRuntime;
use serde_json::Value;

const MATRIX: &str = include_str!("fixtures/web-platform-support-matrix.json");

fn page() -> PageRuntime {
    PageRuntime::from_html(
        "<!doctype html><html><body></body></html>",
        Vec::new(),
        800,
        "https://matrix.test/",
    )
    .expect("fixture page must load")
}

fn matrix() -> Vec<Value> {
    let raw: Value = serde_json::from_str(MATRIX).expect("matrix fixture must be valid JSON");
    raw.get("features")
        .and_then(Value::as_array)
        .cloned()
        .expect("matrix must contain a features array")
}

fn assert_boolean(actual: &JsvValue, expected: bool, probe: &str) {
    match actual.as_boolean() {
        Some(value) => assert_eq!(
            value, expected,
            "probe {probe:?} evaluated to {value}, expected {expected}"
        ),
        None => panic!("probe {probe:?} did not evaluate to a boolean: {actual:?}"),
    }
}

#[test]
fn matrix_schema_is_valid_and_complete() {
    let features = matrix();
    assert!(
        features.len() >= 20,
        "matrix should document a meaningful surface, got {}",
        features.len()
    );
    let mut ids = std::collections::BTreeSet::new();
    for entry in &features {
        let id = entry["id"].as_str().expect("feature id");
        assert!(ids.insert(id.to_string()), "duplicate feature id {id}");
        assert!(entry["name"].is_string(), "{id}: name");
        let status = entry["status"].as_str().unwrap_or_default();
        assert!(
            matches!(status, "implemented" | "unsupported_fail_closed"),
            "{id}: unexpected status {status:?}"
        );
        assert!(entry["probe"].is_string(), "{id}: probe");
        let expect = entry["expect"].as_str().unwrap_or_default();
        assert!(
            matches!(expect, "true" | "false" | "throws"),
            "{id}: unexpected expect {expect:?}"
        );
        assert!(entry["description"].is_string(), "{id}: description");
    }
}

#[test]
fn implemented_features_are_exposed_through_the_real_runtime() {
    let mut page = page();
    for entry in matrix() {
        if entry["status"] != "implemented" {
            continue;
        }
        let id = entry["id"].as_str().unwrap();
        let probe = entry["probe"].as_str().unwrap();
        let expected = entry["expect"].as_str().unwrap() == "true";
        let result = page.evaluate(probe).unwrap_or_else(|error| {
            panic!("{id}: implemented probe {probe:?} must not throw, got {error}")
        });
        assert_boolean(&result, expected, probe);
    }
}

#[test]
fn unsupported_features_fail_closed() {
    let mut page = page();
    for entry in matrix() {
        if entry["status"] != "unsupported_fail_closed" {
            continue;
        }
        let id = entry["id"].as_str().unwrap();
        let probe = entry["probe"].as_str().unwrap();
        match entry["expect"].as_str().unwrap() {
            "throws" => {
                let result = page.evaluate(probe);
                assert!(
                    result.is_err(),
                    "{id}: probe {probe:?} must fail closed (throw), got {result:?}"
                );
            }
            "true" => {
                let result = page.evaluate(probe).unwrap_or_else(|error| {
                    panic!("{id}: fail-closed probe {probe:?} must not throw, got {error}")
                });
                assert_boolean(&result, true, probe);
            }
            other => panic!("{id}: unsupported expect {other:?}"),
        }
    }
}

#[test]
fn fail_closed_errors_are_type_errors_not_silent_undefined() {
    // The window.* fail-closed contract is specifically a TypeError, so
    // scripts detect the missing surface instead of reading undefined.
    let mut page = page();
    let error = page
        .evaluate("window.WebTransport")
        .expect_err("window.WebTransport must throw");
    assert!(
        error.contains("not implemented") || error.contains("TypeError"),
        "expected a not-implemented TypeError, got: {error}"
    );
}
