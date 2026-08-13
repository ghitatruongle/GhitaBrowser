//! Phase 21 acceptance gate: composed Shadow DOM rendering, event
//! retargeting, composed paths and JavaScript attachShadow bindings.

use ghitabrowser::live_dom::LiveDocument;
use ghitabrowser::web_runtime::PageRuntime;

fn page(html: &str) -> PageRuntime {
    let mut page = PageRuntime::from_html(html, Vec::new(), 800, "https://app.test/")
        .expect("page runtime construction must succeed");
    page.run_document().expect("inline scripts must run");
    page
}

#[test]
fn shadow_dom_children_compose_into_the_rendered_tree() {
    let mut page = page(
        "<main><div id='host'></div>\
         <script>let host=document.getElementById('host');\
         let root=host.attachShadow({mode:'open'});\
         root.innerHTML='<p class=\"shadowed\">shadow text</p>';\
         </script></main>",
    );
    let dom = page.dom_element();
    let host = dom
        .find_all_tags("div")
        .into_iter()
        .find(|element| element.get_attr("id").map(String::as_str) == Some("host"))
        .expect("host element must exist");
    // The composed export contains the shadow child under the host.
    assert!(
        host.children
            .iter()
            .any(|child| { child.tag == "p" && child.text.contains("shadow text") }),
        "composed tree must include shadow children: {host:?}"
    );

    // The renderer paints the composed text.
    let render = page.live_document().render_state();
    assert!(
        render
            .display_list
            .items
            .iter()
            .any(|item| matches!(item, ghitabrowser::paint::DisplayItem::TextRun { content, .. } if content.contains("shadow text"))),
        "display list must contain shadow text"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn slots_distribute_light_children_into_the_shadow_tree() {
    let mut page = page(
        "<main><div id='host'><span slot='title'>Light Title</span><span>Fallback</span></div>\
         <script>let host=document.getElementById('host');\
         let root=host.attachShadow({mode:'open'});\
         root.innerHTML='<h1><slot name=\"title\">Default</slot></h1><p><slot>empty</slot></p>';\
         </script></main>",
    );
    let dom = page.dom_element();
    let host = dom
        .find_all_tags("div")
        .into_iter()
        .find(|element| element.get_attr("id").map(String::as_str) == Some("host"))
        .expect("host element must exist");
    let flat = format!("{host:?}");
    assert!(
        flat.contains("Light Title"),
        "slotted light child must render inside the shadow tree: {flat}"
    );
    // The light child without a slot attribute lands in the default slot.
    assert!(
        flat.contains("Fallback"),
        "default slot must take light children: {flat}"
    );
}

#[test]
fn events_retarget_to_the_host_outside_the_shadow_boundary() {
    let mut page = page(
        "<main id='app'><div id='host'></div>\
         <script>let seen='';\
         document.getElementById('host').addEventListener('click',event=>{seen=event.target.id});\
         let root=document.getElementById('host').attachShadow({mode:'open'});\
         root.innerHTML='<button id=\"inner\">Go</button>';\
         </script></main>",
    );
    let inner = {
        // Find the shadow button's live node id through the composed export
        // (the export preserves live node identities).
        let dom = page.dom_element();
        let host = dom
            .find_all_tags("div")
            .into_iter()
            .find(|element| element.get_attr("id").map(String::as_str) == Some("host"))
            .unwrap();
        let button = host
            .children
            .iter()
            .find(|child| child.tag == "button")
            .expect("shadow button in composed tree")
            .clone();
        assert_eq!(button.get_attr("id").map(String::as_str), Some("inner"));
        button.node_id.expect("composed export keeps live node ids")
    };
    let report = page.click(inner).unwrap();
    assert!(report.invoked_listeners >= 1);
    // The host listener observes the retargeted target (the host itself).
    assert_eq!(
        page.evaluate("seen"),
        Ok(ghitabrowser::javascript::JsvValue::String(
            "host".to_string()
        ))
    );
}

#[test]
fn closed_shadow_roots_hide_shadow_root_access() {
    let mut page = page(
        "<main><div id='host'></div>\
         <script>let host=document.getElementById('host');\
         let root=host.attachShadow({mode:'closed'});\
         let visible=host.shadowRoot;\
         </script></main>",
    );
    assert_eq!(
        page.evaluate("visible"),
        Ok(ghitabrowser::javascript::JsvValue::Null),
        "closed shadowRoot must be null from page script"
    );
}

#[test]
fn shadow_dom_limits_apply_to_roots_and_bytes() {
    let page = page(
        "<main><div id='a'></div><div id='b'></div>\
         <script>let a=document.getElementById('a');\
         let first=a.attachShadow({mode:'open'});\
         let duplicate=a.attachShadow({mode:'open'});\
         </script></main>",
    );
    // attachShadow on a host that already has a shadow root fails closed.
    assert!(
        page.report()
            .errors
            .iter()
            .any(|error| error.contains("NotSupportedError")),
        "duplicate attachShadow must fail: {:?}",
        page.report().errors
    );
}

#[test]
fn shadow_dom_dispatch_works_through_live_document_api() {
    // Rust-level API: attach a shadow tree and verify the composed export.
    let mut document = LiveDocument::parse(
        "<main><div id='host'><span slot='x'>slotted</span></div></main>",
        Vec::new(),
        800,
    );
    let host = document.get_element_by_id("host").unwrap();
    let root = document
        .attach_shadow(host, ghitabrowser::live_dom::ShadowMode::Open)
        .unwrap();
    document
        .set_shadow_html(root, "<slot name='x'></slot>")
        .unwrap();
    let dom = document.to_element_public();
    let host = dom
        .find_all_tags("div")
        .into_iter()
        .find(|element| element.get_attr("id").map(String::as_str) == Some("host"))
        .unwrap();
    assert!(format!("{host:?}").contains("slotted"));
}
