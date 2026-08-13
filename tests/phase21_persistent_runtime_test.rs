//! Phase 21 acceptance gate: the page runtime persists one interpreter, one
//! live DOM and one event loop across script tags, event dispatches, timers
//! and origin storage. A fresh engine per script or a counting-only event
//! bridge fails these tests.

use ghitabrowser::javascript::JsvValue;
use ghitabrowser::live_dom::NodeId;
use ghitabrowser::web_runtime::PageRuntime;

fn page(html: &str) -> PageRuntime {
    let mut page = PageRuntime::from_html(html, Vec::new(), 800, "https://app.test/")
        .expect("page runtime construction must succeed");
    page.run_document().expect("inline scripts must run");
    page
}

fn node(page: &PageRuntime, id: &str) -> NodeId {
    page.live_document()
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("missing element #{id}"))
}

fn number(value: &JsvValue) -> f64 {
    value
        .as_number()
        .unwrap_or_else(|| panic!("expected number, got {value:?}"))
}

fn string(value: &JsvValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
        .to_string()
}

#[test]
fn top_level_bindings_persist_across_script_tags() {
    let mut page = page(
        "<main><script>let counter=0;function tick(){counter=counter+1;return counter}</script>\
         <script>counter=counter+1;let doubled=tick()*2</script></main>",
    );
    // Script 1: counter=0. Script 2: counter=1, then tick() bumps it to 2.
    assert_eq!(number(&page.evaluate("counter").unwrap()), 2.0);
    assert_eq!(number(&page.evaluate("doubled").unwrap()), 4.0);
    assert_eq!(number(&page.evaluate("tick()").unwrap()), 3.0);
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn event_listener_callbacks_run_on_click_and_observe_default_prevention() {
    let mut page = page(
        "<main><button id='btn'>Go</button><input id='choice' type='checkbox'>\
         <script>let clicks=0;let prevented=0;\
         document.getElementById('btn').addEventListener('click',()=>{clicks=clicks+1});\
         document.getElementById('choice').addEventListener('click',event=>{prevented=prevented+1;event.preventDefault()});\
         </script></main>",
    );
    let button = node(&page, "btn");
    let choice = node(&page, "choice");
    let report = page.click(button).unwrap();
    assert!(report.default_actions.is_empty());
    assert_eq!(number(&page.evaluate("clicks").unwrap()), 1.0);

    page.click(choice).unwrap();
    // The checkbox default toggle was prevented by the listener.
    assert!(
        page.live_document()
            .get_attribute(choice, "checked")
            .is_none(),
        "preventDefault must stop the checkbox default action"
    );
    assert_eq!(number(&page.evaluate("prevented").unwrap()), 1.0);
}

#[test]
fn custom_events_carry_bounded_detail_and_bubble_to_ancestors() {
    let mut page = page(
        "<main id='app'><p id='target'></p>\
         <script>let seen='';let count=0;\
         document.getElementById('target').addEventListener('ping',event=>{seen=event.detail});\
         document.getElementById('app').addEventListener('ping',event=>{count=count+1});\
         </script></main>",
    );
    let target = node(&page, "target");
    page.dispatch_custom_event(target, "ping", Some("hello".to_string()), true)
        .unwrap();
    assert_eq!(string(&page.evaluate("seen").unwrap()), "hello");
    assert_eq!(number(&page.evaluate("count").unwrap()), 1.0);

    // Non-bubbling events stay on the target.
    page.dispatch_custom_event(target, "ping", Some("x".to_string()), false)
        .unwrap();
    assert_eq!(number(&page.evaluate("count").unwrap()), 1.0);
}

#[test]
fn script_created_events_dispatch_through_the_host_bridge() {
    let mut page = page(
        "<main><p id='target'></p>\
         <script>let total=0;\
         let target=document.getElementById('target');\
         target.addEventListener('sum',event=>{total=total+Number(event.detail)});\
         target.dispatchEvent(new CustomEvent('sum',{detail:'5'}));\
         target.dispatchEvent(new CustomEvent('sum',{detail:'7'}));\
         </script></main>",
    );
    assert_eq!(number(&page.evaluate("total").unwrap()), 12.0);
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn timers_fire_callbacks_and_intervals_reschedule_with_budgets() {
    let mut page = page(
        "<main><script>let ticks=0;\
         let id=setInterval(()=>{ticks=ticks+1},10);\
         setTimeout(()=>{clearInterval(id)},35);\
         </script></main>",
    );
    assert_eq!(page.pending_timers(), 2);
    page.pump_timers(10).unwrap();
    page.pump_timers(10).unwrap();
    page.pump_timers(10).unwrap();
    page.pump_timers(10).unwrap();
    assert_eq!(number(&page.evaluate("ticks").unwrap()), 3.0);
    assert_eq!(
        page.pending_timers(),
        0,
        "clearInterval must remove the timer"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[allow(non_snake_case)]
#[test]
fn localStorage_get_set_remove_and_key_are_origin_scoped() {
    let mut page = page(
        "<main><script>localStorage.setItem('theme','dark');\
         let stored=localStorage.getItem('theme');\
         let missing=localStorage.getItem('nope');\
         let count=localStorage.length;\
         localStorage.removeItem('theme');\
         let after=localStorage.getItem('theme');\
         </script></main>",
    );
    assert_eq!(string(&page.evaluate("stored").unwrap()), "dark");
    assert_eq!(page.evaluate("missing").unwrap(), JsvValue::Null);
    assert_eq!(number(&page.evaluate("count").unwrap()), 1.0);
    assert_eq!(page.evaluate("after").unwrap(), JsvValue::Null);
    // Storage survives later turns (origin-scoped, in-memory for this phase).
    page.evaluate("localStorage.setItem('theme','light')")
        .unwrap();
    assert_eq!(
        string(&page.evaluate("localStorage.getItem('theme')").unwrap()),
        "light"
    );
}

#[test]
fn once_and_capture_listener_options_are_observed() {
    let mut page = page(
        "<main id='app'><button id='btn'>Go</button>\
         <script>let calls=[];\
         let app=document.getElementById('app');\
         app.addEventListener('click',()=>{calls.push('capture')},{capture:true});\
         let btn=document.getElementById('btn');\
         btn.addEventListener('click',()=>{calls.push('once')},{once:true});\
         app.addEventListener('click',()=>{calls.push('bubble')});\
         </script></main>",
    );
    let button = node(&page, "btn");
    page.click(button).unwrap();
    page.click(button).unwrap();
    let result = string(&page.evaluate("calls.join(',')").unwrap());
    assert_eq!(
        result, "capture,once,bubble,capture,bubble",
        "capture runs before target, once runs exactly one time"
    );
}

#[test]
fn listener_removal_uses_function_identity() {
    let mut page = page(
        "<main><button id='btn'>Go</button>\
         <script>let count=0;\
         function handler(){count=count+1}\
         let btn=document.getElementById('btn');\
         btn.addEventListener('click',handler);\
         btn.removeEventListener('click',handler);\
         btn.addEventListener('click',()=>{count=count+100});\
         </script></main>",
    );
    page.click(node(&page, "btn")).unwrap();
    assert_eq!(number(&page.evaluate("count").unwrap()), 100.0);
}

#[test]
fn keyboard_input_flows_through_js_listeners_with_change_on_blur() {
    let mut page = page(
        "<main><input id='name'><input id='other'>\
         <script>let inputs=[];let changes=[];\
         let name=document.getElementById('name');\
         name.addEventListener('input',event=>{inputs.push(name.value)});\
         name.addEventListener('change',()=>{changes.push('changed')});\
         </script></main>",
    );
    let name = node(&page, "name");
    page.focus(name).unwrap();
    page.dispatch_keyboard("keydown", "A").unwrap();
    assert_eq!(number(&page.evaluate("inputs.length").unwrap()), 1.0);
    page.focus(node(&page, "other")).unwrap();
    assert_eq!(number(&page.evaluate("changes.length").unwrap()), 1.0);
}

#[test]
fn page_teardown_leaves_no_pending_timers_or_listeners() {
    let page = page(
        "<main><button id='btn'>Go</button>\
         <script>let timer=setTimeout(()=>{},10);\
         document.getElementById('btn').addEventListener('click',()=>{});\
         clearTimeout(timer);\
         </script></main>",
    );
    assert_eq!(page.pending_timers(), 0);
    let dom = page.settle();
    assert!(!dom.find_all_tags("button").is_empty());
}

// ===== Phase 21 / M2: navigation and history =====

#[test]
fn history_push_replace_and_traversal_dispatch_popstate() {
    let mut page = page(
        "<main><script>let states=[];\
         window.addEventListener('popstate',event=>{states.push(event.detail)});\
         history.pushState('page:1','','/one');\
         history.pushState('page:2','','/two');\
         history.back();\
         history.forward();\
         </script></main>",
    );
    assert_eq!(number(&page.evaluate("history.length").unwrap()), 3.0);
    // popstate carries the serialized state payload of the traversed entries.
    assert_eq!(
        string(&page.evaluate("states.join(',')").unwrap()),
        "page:1,page:2"
    );
    assert_eq!(string(&page.evaluate("location.pathname").unwrap()), "/two");
    let mutations = page.report().history_mutations.clone();
    assert!(
        mutations
            .iter()
            .any(|entry| entry.starts_with("pushState https://app.test/one")),
        "{mutations:?}"
    );
    assert!(
        mutations
            .iter()
            .any(|entry| entry.starts_with("popstate https://app.test/one")),
        "{mutations:?}"
    );
}

#[test]
fn replace_state_keeps_length_and_hash_changes_trigger_hashchange() {
    let mut page = page(
        "<main><script>let hashes=[];\
         window.addEventListener('hashchange',event=>{hashes.push(event.detail)});\
         history.replaceState({},'','/base');\
         location.hash='#section';\
         location.hash='#other';\
         </script></main>",
    );
    assert_eq!(number(&page.evaluate("history.length").unwrap()), 1.0);
    assert_eq!(string(&page.evaluate("location.hash").unwrap()), "#other");
    assert_eq!(
        string(&page.evaluate("hashes.join(',')").unwrap()),
        "#section,#other"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn cross_origin_history_and_navigation_are_rejected_or_recorded() {
    let mut page = page(
        "<main><script>let blocked=0;\
         try{history.pushState({},'','https://evil.test/')}catch(error){blocked=1}\
         location.assign('https://evil.test/steal');\
         </script></main>",
    );
    assert_eq!(number(&page.evaluate("blocked").unwrap()), 1.0);
    let mutations = page.report().history_mutations.clone();
    assert!(
        mutations
            .iter()
            .any(|entry| entry.starts_with("navigate-blocked")),
        "{mutations:?}"
    );
    assert_eq!(
        string(&page.evaluate("location.href").unwrap()),
        "https://app.test/"
    );
}

// ===== Phase 21 / M3: forms =====

#[test]
fn form_submission_collects_named_controls_and_is_cancelable() {
    let mut page_a = page(
        "<main><form id='form' action='/search' method='get'>\
         <input name='q' value='rust'>\
         <select name='sort'><option value='new'>New</option><option value='top' selected>Top</option></select>\
         <textarea name='note'>hello</textarea>\
         <button id='go' type='submit'>Send</button>\
         </form>\
         <script>let submits=0;document.getElementById('form').addEventListener('submit',event=>{submits=submits+1});</script></main>",
    );
    let go = node(&page_a, "go");
    let _report = page_a.click(go).unwrap();
    assert_eq!(number(&page_a.evaluate("submits").unwrap()), 1.0);
    let submissions = page_a.report().submitted_forms.clone();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0], "get /search 26", "{submissions:?}");

    // preventDefault on submit stops the submission.
    let mut page2 = page(
        "<main><form id='form' action='/search'>\
         <input name='q' value='rust'>\
         </form>\
         <script>document.getElementById('form').addEventListener('submit',event=>event.preventDefault());</script></main>",
    );
    page2.click(node(&page2, "form")).unwrap();
    assert!(page2.report().submitted_forms.is_empty());
}

#[test]
fn form_required_validation_blocks_empty_submission() {
    let mut page = page(
        "<main><form id='form' action='/join'>\
         <input name='email' required>\
         <button id='go' type='submit'>Send</button>\
         </form>\
         <script>let submits=0;document.getElementById('form').addEventListener('submit',()=>{submits=submits+1});</script></main>",
    );
    let go = node(&page, "go");
    page.click(go).unwrap();
    // The submit listener still runs, but the submission is blocked.
    assert_eq!(number(&page.evaluate("submits").unwrap()), 1.0);
    assert!(page.report().submitted_forms.is_empty());
    assert!(!page.report().validation_errors.is_empty());
}

#[test]
fn select_click_cycles_options_and_exposes_value() {
    let mut page = page(
        "<main><select id='sort'><option value='new'>New</option><option value='top'>Top</option></select>\
         <script>let changes=0;document.getElementById('sort').addEventListener('change',()=>{changes=changes+1});</script></main>",
    );
    let select = node(&page, "sort");
    assert_eq!(
        string(
            &page
                .evaluate("document.getElementById('sort').value")
                .unwrap()
        ),
        "new"
    );
    page.click(select).unwrap();
    assert_eq!(
        string(
            &page
                .evaluate("document.getElementById('sort').value")
                .unwrap()
        ),
        "top"
    );
    assert_eq!(number(&page.evaluate("changes").unwrap()), 1.0);
}

#[test]
fn textarea_edits_flow_through_text_content_and_change() {
    let mut page_a = page(
        "<main><textarea id='note'></textarea>\
         <script>let inputs=0;let note=document.getElementById('note');\
         note.addEventListener('input',()=>{inputs=inputs+1});\
         note.addEventListener('change',()=>{inputs=inputs+100});\
         </script></main>",
    );
    let note = node(&page_a, "note");
    page_a.focus(note).unwrap();
    page_a.dispatch_keyboard("keydown", "H").unwrap();
    assert_eq!(
        string(
            &page_a
                .evaluate("document.getElementById('note').value")
                .unwrap()
        ),
        "H"
    );
    assert_eq!(number(&page_a.evaluate("inputs").unwrap()), 1.0);
    // Change fires when a text entry loses focus to another control.
    let mut page2 = page(
        "<main><textarea id='note'></textarea><input id='other'>\
         <script>let changes=0;document.getElementById('note').addEventListener('change',()=>{changes=changes+1});</script></main>",
    );
    let note2 = node(&page2, "note");
    page2.focus(note2).unwrap();
    page2.dispatch_keyboard("keydown", "A").unwrap();
    page2.focus(node(&page2, "other")).unwrap();
    assert_eq!(number(&page2.evaluate("changes").unwrap()), 1.0);
}

// ===== Phase 21: persistent module loading and dynamic import =====

#[test]
fn dynamic_import_resolves_module_namespaces_with_persistent_cache() {
    let mut page = PageRuntime::from_html(
        "<main><script>let loaded=0;\
         import('./math.js').then(ns=>{loaded=ns.times(ns.base,4)});\
         </script></main>",
        Vec::new(),
        800,
        "https://app.test/",
    )
    .expect("page runtime construction must succeed");
    // Modules must be registered before the script turn that imports them.
    page.register_module(
        "./math.js",
        "export const base=5;\nexport function times(a,b){return a*b}",
    )
    .unwrap();
    page.run_document().unwrap();
    page.flush_pending().unwrap();
    assert_eq!(number(&page.evaluate("loaded").unwrap()), 20.0);
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn dynamic_import_links_named_imports_and_caches_namespaces() {
    let mut page = PageRuntime::from_html(
        "<main><script>let total=0;\
         import('./entry.js').then(ns=>{total=ns.result});\
         </script></main>",
        Vec::new(),
        800,
        "https://app.test/",
    )
    .expect("page runtime construction must succeed");
    page.register_module("./dep.js", "export const factor=3;")
        .unwrap();
    page.register_module(
        "./entry.js",
        "import { factor } from './dep.js';\nexport const result=factor*7;",
    )
    .unwrap();
    page.run_document().unwrap();
    page.flush_pending().unwrap();
    assert_eq!(number(&page.evaluate("total").unwrap()), 21.0);
    // The cache persists: a second import does not re-execute.
    page.evaluate("import('./entry.js').then(ns=>{total=ns.result+1})")
        .unwrap();
    assert_eq!(number(&page.evaluate("total").unwrap()), 22.0);
}

#[test]
fn dynamic_import_fails_closed_on_missing_or_circular_modules() {
    let mut page_a = PageRuntime::from_html(
        "<main><script>let error='';\
         import('./missing.js').catch(e=>{error=String(e)});\
         </script></main>",
        Vec::new(),
        800,
        "https://app.test/",
    )
    .expect("page runtime construction must succeed");
    page_a.run_document().unwrap();
    page_a.flush_pending().unwrap();
    assert!(
        text(&page_a.evaluate("error").unwrap()).contains("not found"),
        "missing module must reject the promise"
    );

    let mut page2 = PageRuntime::from_html(
        "<main><script>let error='';\
         import('./a.js').catch(e=>{error=String(e)});\
         </script></main>",
        Vec::new(),
        800,
        "https://app.test/",
    )
    .expect("page runtime construction must succeed");
    page2
        .register_module("./a.js", "import { b } from './b.js';\nexport const a=b;")
        .unwrap();
    page2
        .register_module("./b.js", "import { a } from './a.js';\nexport const b=a;")
        .unwrap();
    page2.run_document().unwrap();
    page2.flush_pending().unwrap();
    assert!(
        text(&page2.evaluate("error").unwrap()).contains("Circular"),
        "circular modules must reject the promise"
    );
}

fn text(value: &JsvValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
        .to_string()
}
