use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ghitabrowser::app_platform::{
    ApplicationDocument, CustomElementDefinition, LifecycleKind, ShadowMode,
};
use ghitabrowser::javascript::JsvValue;
use ghitabrowser::live_dom::ListenerOptions;

#[test]
fn offline_spa_hydrates_modules_templates_shadow_slots_and_stays_interactive() {
    let html = r#"
        <main id="app">
          <template id="action-row"><button class="action">Increment</button></template>
          <x-counter count="0"><strong slot="label">Offline counter</strong></x-counter>
        </main>
    "#;
    let mut app = ApplicationDocument::parse(html, vec![], 960);
    let counter_definition =
        CustomElementDefinition::new("x-counter", ["count"]).expect("valid custom element");
    assert_eq!(app.define_custom_element(counter_definition).unwrap(), 1);

    let counter = app.document().query_selector("x-counter").unwrap();
    app.attach_shadow(
        counter,
        ShadowMode::Open,
        "<section class='counter'><slot name='label'></slot><slot></slot></section>",
    )
    .unwrap();
    let shadow = app.shadow_root(counter).unwrap();
    assert_eq!(shadow.slot_assignments()[0].assigned_nodes.len(), 1);

    assert!(app.document().query_selector("button.action").is_none());
    let inserted = app.instantiate_template("action-row", counter).unwrap();
    assert_eq!(inserted.len(), 1);
    let button = app.document().query_selector("button.action").unwrap();

    app.register_module("./state.js", "export const initial = 40;")
        .unwrap();
    app.register_module(
        "./app.js",
        "import { initial } from './state.js';\nexport const hydrated = initial + 2;",
    )
    .unwrap();
    let namespace = app.evaluate_module("./app.js").unwrap();
    assert_eq!(
        namespace.exports.get("hydrated"),
        Some(&JsvValue::Number(42.0))
    );

    let clicks = Arc::new(AtomicUsize::new(0));
    let observed_clicks = Arc::clone(&clicks);
    app.document_mut()
        .add_event_listener(
            button,
            "click",
            ListenerOptions::default(),
            Arc::new(move |_| {
                observed_clicks.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();

    let report = app.hydrate();
    assert!(app.is_hydrated());
    assert_eq!(report.custom_elements, 1);
    assert_eq!(report.shadow_roots, 1);
    assert_eq!(report.templates, 1);
    assert_eq!(report.evaluated_modules, 1);

    app.document_mut().click(button).unwrap();
    app.document_mut().click(button).unwrap();
    assert_eq!(clicks.load(Ordering::SeqCst), 2);

    let revision_before = app.document().render_state().revision;
    app.set_attribute(counter, "count", "2").unwrap();
    app.document_mut().refresh();
    assert!(app.document().render_state().revision > revision_before);
    assert!(app.lifecycle_records().iter().any(|record| {
        matches!(
            &record.kind,
            LifecycleKind::AttributeChanged {
                name,
                new_value: Some(value),
                ..
            } if name == "count" && value == "2"
        )
    }));

    let root = app.document().root();
    app.remove_child(root, counter).unwrap();
    app.append_child(root, counter).unwrap();
    let lifecycle = app.take_lifecycle_records();
    assert!(lifecycle
        .iter()
        .any(|record| matches!(record.kind, LifecycleKind::Disconnected)));
    assert!(lifecycle
        .iter()
        .any(|record| matches!(record.kind, LifecycleKind::Connected)));

    app.document_mut().click(button).unwrap();
    assert_eq!(clicks.load(Ordering::SeqCst), 3);
}

#[test]
fn closed_shadow_root_is_hidden_from_dom_facing_accessor() {
    let mut app = ApplicationDocument::parse("<secure-shell></secure-shell>", vec![], 800);
    let host = app.document().query_selector("secure-shell").unwrap();
    app.attach_shadow(host, ShadowMode::Closed, "<p>private</p>")
        .unwrap();
    assert!(app.shadow_root(host).is_none());
    assert!(app.shadow_root_internal(host).is_some());
}
