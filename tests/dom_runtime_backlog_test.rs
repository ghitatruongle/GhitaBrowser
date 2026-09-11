// Regression tests for the 2026-08-30 review backlog (live-DOM depth/free,
// RAF/event budgets, innerHTML round-trip, parser comment/attribute edges).

use ghitabrowser::parser::parse_html;
use ghitabrowser::renderer::render_to_string;
use ghitabrowser::web_runtime::PageRuntime;

fn runtime(html: &str) -> PageRuntime {
    PageRuntime::from_html(html, Vec::new(), 800, "https://example.test/").expect("runtime")
}

#[test]
fn deep_append_child_chain_hits_the_depth_budget_instead_of_overflowing() {
    // Direct API: refresh/layout/drop recursion is recursive, so an
    // unbounded depth must be rejected long before it can overflow the
    // stack (the old code aborted the process).
    use ghitabrowser::live_dom::LiveDocument;
    let mut doc = LiveDocument::parse("<div id='a'></div>", Vec::new(), 800);
    let root = doc.root();
    let mut parent = doc.query_selector("#a").expect("anchor exists");
    let mut tripped = None;
    for _ in 0..400 {
        let child = doc.create_element("div").expect("create element");
        match doc.append_child(parent, child) {
            Ok(()) => parent = child,
            Err(error) => {
                tripped = Some(error);
                break;
            }
        }
    }
    let error = tripped.expect("the depth budget never engaged");
    assert!(
        error.contains("depth") || error.contains("Hierarchy"),
        "unexpected error: {error}"
    );
    // The document remains usable: root still resolves.
    assert_eq!(doc.root(), root);
}

#[test]
fn innerhtml_churn_frees_detached_nodes_instead_of_killing_the_page() {
    let mut page = runtime("<body></body>");
    // 300 turns x ~200 nodes = 60k cumulative allocations, far past the
    // 50k node quota that used to be permanent once reached.
    for _ in 0..300 {
        page.evaluate(
            "var s = ''; var i = 0; while (i < 200) { s = s + '<div></div>'; i = i + 1; } document.body.innerHTML = s; 1",
        )
        .unwrap();
    }
    // The page still renders: the getter returns the fresh markup and the
    // node quota never tripped (60k cumulative allocations, 50k quota).
    let html = page
        .evaluate("document.body.innerHTML")
        .expect("page must stay alive past the old 50k quota");
    assert!(html.to_display_string().contains("<div>"));
}

#[test]
fn innerhtml_getter_round_trips_nested_markup() {
    let mut page = runtime("<div id='x'><b>hi</b></div>");
    let html = page
        .evaluate(
            "var el = document.getElementById('x'); el.innerHTML = el.innerHTML; el.innerHTML",
        )
        .unwrap();
    assert!(
        html.to_display_string().contains("<b>"),
        "markup destroyed by round-trip: {html:?}"
    );
}

#[test]
fn request_animation_frame_registration_is_capped() {
    let mut page = runtime("<body></body>");
    let count = page
        .evaluate(
            "var i = 0; try { while (i < 1100) { requestAnimationFrame(function () {}); i = i + 1; } } catch (e) {} i",
        )
        .unwrap();
    assert_eq!(count, ghitabrowser::javascript::JsvValue::Number(1024.0));
}

#[test]
fn event_allocation_evicts_oldest_instead_of_breaking_the_api() {
    let mut page = runtime("<body></body>");
    let count = page
        .evaluate("var i = 0; while (i < 300) { new Event('x'); i = i + 1; } i")
        .unwrap();
    assert_eq!(count, ghitabrowser::javascript::JsvValue::Number(300.0));
}

#[test]
fn empty_comment_does_not_swallow_the_rest_of_the_document() {
    // "<!-->" is a complete empty comment per HTML5.
    let tree = parse_html("a<!-->b");
    let html = render_to_string(&tree);
    assert!(
        html.contains('a') && html.contains('b'),
        "lost content: {html}"
    );
}

#[test]
fn attribute_name_scanner_stops_at_angle_bracket() {
    // `<div<p>` must create a nested <p>, not an attribute named "<p".
    fn find<'a>(
        node: &'a ghitabrowser::parser::Element,
        tag: &str,
    ) -> Option<&'a ghitabrowser::parser::Element> {
        if node.tag == tag {
            return Some(node);
        }
        node.children.iter().find_map(|child| find(child, tag))
    }
    let tree = parse_html("<div<p>hello</div>");
    let div = find(&tree, "div").expect("div present");
    assert!(find(div, "p").is_some(), "nested <p> swallowed: {tree:?}");
}
