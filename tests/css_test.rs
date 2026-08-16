//! Phase 21 acceptance gate: CSS cascade/layout expansion — custom
//! properties (`var()`), the bounded `@media` subset and combinator
//! selectors (`div p`, `div > p`, `:root`).

use ghitabrowser::css_parser::{
    compute_computed_style, compute_computed_style_with_ancestors, parse_css, parse_css_with_media,
    ElementAncestry,
};
use std::collections::HashMap;

fn attrs(map: &[(&str, &str)]) -> HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn custom_properties_resolve_through_var_and_fallback() {
    let rules = parse_css(
        ":root { --accent: #ff5500; --spacing: 8px; } \
         .card { color: var(--accent); padding: var(--spacing); } \
         .missing { color: var(--nope, #123456); }",
    );
    let card = compute_computed_style_with_ancestors(
        "div",
        &["card".to_string()],
        None,
        &rules,
        None,
        &attrs(&[]),
        true,
        &[],
    );
    assert_eq!(card.color.as_deref(), Some("#ff5500"));
    assert_eq!(
        card.padding_top,
        Some(ghitabrowser::css_parser::CssUnit::Pixels(8.0))
    );

    let missing = compute_computed_style(
        "div",
        &["missing".to_string()],
        None,
        &rules,
        None,
        &attrs(&[]),
    );
    assert_eq!(missing.color.as_deref(), Some("#123456"));
}

#[test]
fn custom_properties_inherit_through_the_cascade() {
    let rules = parse_css(":root { --theme: dark; } .child { color: var(--theme); }");
    let parent = compute_computed_style_with_ancestors(
        "main",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        true,
        &[],
    );
    let child = compute_computed_style_with_ancestors(
        "div",
        &["child".to_string()],
        None,
        &rules,
        Some(&parent),
        &attrs(&[]),
        false,
        &[],
    );
    assert_eq!(child.color.as_deref(), Some("dark"));
}

#[test]
fn media_queries_select_rules_by_viewport_width() {
    let css = "@media (max-width: 600px) { .mobile { display: block; } } \
               @media (min-width: 601px) { .desktop { display: grid; } } \
               .base { color: red; }";
    let narrow = parse_css_with_media(css, 480);
    assert_eq!(narrow.len(), 2, "mobile + base apply at 480px");
    let wide = parse_css_with_media(css, 1024);
    assert_eq!(wide.len(), 2, "desktop + base apply at 1024px");

    // `screen and (...)` and comma-OR lists also work.
    let combined = parse_css_with_media(
        "@media screen and (max-width: 100px), (min-width: 900px) { .x { color: blue; } }",
        50,
    );
    assert_eq!(combined.len(), 1);
    let combined = parse_css_with_media(
        "@media screen and (max-width: 100px), (min-width: 900px) { .x { color: blue; } }",
        500,
    );
    assert_eq!(combined.len(), 0, "neither branch matches at 500px");

    // viewport 0 (legacy parse_css) keeps media rules skipped.
    assert_eq!(parse_css(css).len(), 1);
}

#[test]
fn descendant_and_child_combinators_match_against_ancestry() {
    let rules = parse_css(
        "nav a { color: green; } \
         .menu > li { display: list-item; } \
         #app p.hot { color: orange; }",
    );
    let ancestry = vec![
        ElementAncestry {
            tag: "nav".to_string(),
            classes: vec![],
            id: None,
        },
        ElementAncestry {
            tag: "body".to_string(),
            classes: vec![],
            id: None,
        },
    ];
    let link = compute_computed_style_with_ancestors(
        "a",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        false,
        &ancestry,
    );
    assert_eq!(link.color.as_deref(), Some("green"));

    // Without the `nav` ancestor the rule must not apply (fail closed).
    let link = compute_computed_style_with_ancestors(
        "a",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        false,
        &[],
    );
    assert_eq!(link.color.as_deref(), Some("#000000"));

    // Child combinator: `.menu > li` matches only direct children.
    let direct = compute_computed_style_with_ancestors(
        "li",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        false,
        &[ElementAncestry {
            tag: "ul".to_string(),
            classes: vec!["menu".to_string()],
            id: None,
        }],
    );
    assert_eq!(direct.display.as_deref(), Some("list-item"));

    // `#app p.hot` needs the id on an ancestor.
    let hot = compute_computed_style_with_ancestors(
        "p",
        &["hot".to_string()],
        None,
        &rules,
        None,
        &attrs(&[]),
        false,
        &[ElementAncestry {
            tag: "div".to_string(),
            classes: vec![],
            id: Some("app".to_string()),
        }],
    );
    assert_eq!(hot.color.as_deref(), Some("orange"));
}

#[test]
fn root_pseudo_class_only_applies_to_the_document_root() {
    let rules = parse_css(":root { background-color: #111111; }");
    let root = compute_computed_style_with_ancestors(
        "main",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        true,
        &[],
    );
    assert_eq!(root.background_color.as_deref(), Some("#111111"));
    let child = compute_computed_style_with_ancestors(
        "div",
        &[],
        None,
        &rules,
        None,
        &attrs(&[]),
        false,
        &[],
    );
    assert_eq!(child.background_color, None);
}

#[test]
fn unresolved_var_references_fail_closed() {
    let rules = parse_css(".broken { width: var(--undefined); }");
    let style = compute_computed_style(
        "div",
        &["broken".to_string()],
        None,
        &rules,
        None,
        &attrs(&[]),
    );
    // The declaration is invalid at computed-value time: no width set.
    assert_eq!(style.width, None);
}
