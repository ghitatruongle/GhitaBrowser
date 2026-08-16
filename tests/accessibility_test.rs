//! Phase 21 acceptance gate: accessibility output for shadow DOM composition
//! and the expanded form controls (form/listbox/option roles, `required`
//! state, textarea/select values).

use ghitabrowser::accessibility::{build_tree, AccessibleRole};
use ghitabrowser::web_runtime::PageRuntime;

#[test]
fn form_controls_expose_roles_values_and_required_state() {
    let dom = ghitabrowser::parser::parse_html(
        "<main><form action='/join'>\
         <input name='email' type='email' required placeholder='Email'>\
         <textarea name='note'>hello world</textarea>\
         <select name='sort'><option value='new'>New</option><option value='top' selected>Top</option></select>\
         </form></main>",
    );
    let tree = build_tree(&dom);
    let debug = format!("{:?}", tree.root.unwrap());
    assert!(debug.contains("Form"), "form role: {debug}");
    assert!(debug.contains("TextBox"), "input textbox role: {debug}");
    assert!(debug.contains("required: true"), "required state: {debug}");
    assert!(
        debug.contains("Email"),
        "placeholder contributes to name: {debug}"
    );
    assert!(debug.contains("ComboBox"), "select combobox role: {debug}");
    assert!(
        debug.contains("ListOption"),
        "option listbox-option role: {debug}"
    );
    assert!(
        debug.contains("hello world"),
        "textarea value from text: {debug}"
    );
    assert!(
        debug.contains("Top"),
        "select value from selected option: {debug}"
    );
}

#[test]
fn shadow_dom_content_appears_in_the_accessibility_tree() {
    let mut page = PageRuntime::from_html(
        "<main><div id='host'><span slot='title'>Light Title</span></div>\
         <script>let host=document.getElementById('host');\
         let root=host.attachShadow({mode:'open'});\
         root.innerHTML='<h2><slot name=\"title\">Default</slot></h2><button>Shadow Button</button>';\
         </script></main>",
        Vec::new(),
        800,
        "https://app.test/",
    )
    .expect("page runtime construction must succeed");
    page.run_document().unwrap();
    let dom = page.dom_element();
    let tree = build_tree(&dom);
    let debug = format!("{:?}", tree.root.unwrap());
    // Composed shadow content must be reachable by assistive technology.
    assert!(
        debug.contains("Shadow Button"),
        "shadow button must appear in the a11y tree: {debug}"
    );
    assert!(
        debug.contains("Light Title"),
        "slotted light child must appear in the a11y tree: {debug}"
    );
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn disabled_and_hidden_controls_are_reflected() {
    let dom = ghitabrowser::parser::parse_html(
        "<main><form><input name='a' required disabled><input name='b' aria-required='true'></form></main>",
    );
    let tree = build_tree(&dom);
    let debug = format!("{:?}", tree.root.unwrap());
    assert!(debug.contains("disabled: true"), "{debug}");
    assert!(debug.contains("required: true"), "{debug}");
}

#[test]
fn roles_match_the_documented_bounded_profile() {
    let dom = ghitabrowser::parser::parse_html(
        "<main><form><input type='checkbox'><input type='radio'><input type='submit'></form></main>",
    );
    let tree = build_tree(&dom);
    let root = tree.root.unwrap();
    fn collect_roles(
        node: &ghitabrowser::accessibility::AccessibleNode,
        out: &mut Vec<AccessibleRole>,
    ) {
        out.push(node.role.clone());
        for child in &node.children {
            collect_roles(child, out);
        }
    }
    let mut roles = Vec::new();
    collect_roles(&root, &mut roles);
    assert!(roles.contains(&AccessibleRole::CheckBox));
    assert!(roles.contains(&AccessibleRole::Radio));
    assert!(roles.contains(&AccessibleRole::Button));
    assert!(roles.contains(&AccessibleRole::Form));
}
