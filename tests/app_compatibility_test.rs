// acceptance gate: an original offline representative application
// fixture exercises the persistent page runtime, events, forms, history,
// composed Shadow DOM, Canvas 2D and the expanded ECMAScript surface
// together through the real product path.

use ghitabrowser::javascript::JsvValue;
use ghitabrowser::paint::{DisplayItem, VectorShapeKind};
use ghitabrowser::web_runtime::PageRuntime;

const FIXTURE: &str = include_str!("fixtures/apps/sample-app.html");

fn number(value: &JsvValue) -> f64 {
    value
        .as_number()
        .unwrap_or_else(|| panic!("expected number, got {value:?}"))
}

fn text(value: &JsvValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
        .to_string()
}

fn app() -> PageRuntime {
    let mut page = PageRuntime::from_html(FIXTURE, Vec::new(), 800, "https://app.test/")
        .expect("fixture must load");
    page.run_document().expect("fixture scripts must run");
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
    page
}

#[test]
fn fixture_runs_without_script_errors() {
    let page = app();
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
    assert_eq!(page.report().scripts_executed, 3);
}

#[test]
fn persistent_runtime_keeps_bindings_across_script_tags() {
    let mut page = app();
    // Script 2 incremented a binding defined in script 1.
    assert_eq!(number(&page.evaluate("session.count").unwrap()), 1.0);
    // Modern ECMAScript results computed during load.
    assert_eq!(text(&page.evaluate("greeting").unwrap()), "hello phase21");
    assert_eq!(number(&page.evaluate("maybe?.deep?.value").unwrap()), 7.0);
    assert_eq!(text(&page.evaluate("label").unwrap()), "B");
    assert_eq!(
        number(&page.evaluate("JSON.parse('{\"k\":3}').k").unwrap()),
        3.0
    );
}

#[test]
fn composed_shadow_dom_renders_the_fixture() {
    let mut page = app();
    let render = page.refresh_render();
    assert!(
        render
            .display_list
            .items
            .iter()
            .any(|item| matches!(item, DisplayItem::TextRun { content, .. } if content.contains("shadow body"))),
        "shadow content must render"
    );
    let dom = page.dom_element();
    let flat = format!("{dom:?}");
    assert!(
        flat.contains("Phase 21"),
        "slotted title must render: {flat}"
    );
}

#[test]
fn canvas_and_css_custom_properties_render_together() {
    let mut page = app();
    let render = page.refresh_render();
    let shapes: Vec<&DisplayItem> = render
        .display_list
        .items
        .iter()
        .filter(|item| matches!(item, DisplayItem::VectorShape(_)))
        .collect();
    assert_eq!(shapes.len(), 1, "fixture draws one canvas rect: {shapes:?}");
    let DisplayItem::VectorShape(shape) = shapes[0] else {
        unreachable!()
    };
    assert_eq!(shape.kind, VectorShapeKind::Rect);
    assert_eq!(shape.w, 40.0);
}

#[test]
fn events_history_and_forms_are_interactive() {
    let mut page = app();
    // Click on the shadow host (retargeted listener on the host).
    let host = page
        .live_document()
        .get_element_by_id("host")
        .expect("host element");
    page.click(host).unwrap();
    // Form submission through the submit button.
    let go = page
        .live_document()
        .get_element_by_id("go")
        .expect("submit button");
    let report = page.click(go).unwrap();
    assert!(
        !report.default_actions.is_empty(),
        "submit button must produce a form action"
    );
    // History traversal emits popstate with the serialized state.
    page.evaluate("history.back()").unwrap();
    let output = text(
        &page
            .evaluate(
                "log.join('
')",
            )
            .unwrap(),
    );
    assert!(
        output.contains("click=host"),
        "click record missing: {output}"
    );
    assert!(
        output.contains("submitted=rust"),
        "submit record missing: {output}"
    );
    assert!(
        output.contains("popstate=s0"),
        "popstate record missing: {output}"
    );
    // Script-set `.value` does not fire input (matching JS semantics);
    // keyboard entry through the focused control does.
    let query = page
        .live_document()
        .get_element_by_id("query")
        .expect("query input");
    page.focus(query).unwrap();
    page.dispatch_keyboard("keydown", "!").unwrap();
    let output = text(
        &page
            .evaluate(
                "log.join('
')",
            )
            .unwrap(),
    );
    assert!(
        output.contains("input=rust!"),
        "input record missing: {output}"
    );
    assert!(
        output.contains("persistent=1:phase21"),
        "persistence record missing: {output}"
    );
}

#[test]
fn media_query_rules_apply_with_the_fixture_viewport() {
    // The fixture stylesheet uses a (max-width: 600px) media query; at 800px
    // the base border width applies.
    let rules = ghitabrowser::css_parser::parse_css_with_media(
        ".card { border-width: 1px; } @media (max-width: 600px) { .card { border-width: 2px; } }",
        800,
    );
    assert_eq!(rules.len(), 1);
    let narrow = ghitabrowser::css_parser::parse_css_with_media(
        ".card { border-width: 1px; } @media (max-width: 600px) { .card { border-width: 2px; } }",
        480,
    );
    assert_eq!(narrow.len(), 2);
}
