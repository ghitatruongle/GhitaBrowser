use ghitabrowser::css_parser::{
    compute_computed_style, eval_math_expression, get_css_diagnostics, parse_css, parse_css_color,
    parse_css_with_media, record_unsupported_property, reset_css_diagnostics,
    supports_query_matches, CssUnit, ElementAncestry, ElementMatchingContext, Selector,
};
use std::collections::HashMap;

#[test]
fn test_compound_selector_and_specificity() {
    let sel = Selector::parse("div.card.highlight#main-card[data-active='true']:hover");
    assert_eq!(sel.tag, Some("div".to_string()));
    assert_eq!(sel.id, Some("main-card".to_string()));
    assert!(
        sel.class.as_deref() == Some("card")
            || sel.components[0].classes.contains(&"highlight".to_string())
    );

    // Specificity: 1 ID (#main-card), 3 Classes/Attrs/Pseudos (.card, .highlight, [data-active], :hover = 4), 1 Tag (div)
    let spec = sel.specificity();
    assert_eq!(spec.0, 1, "IDs should be 1");
    assert!(
        spec.1 >= 3,
        "Classes/Attrs/Pseudos should be >= 3, got {}",
        spec.1
    );
    assert_eq!(spec.2, 1, "Tags should be 1");
}

#[test]
fn test_attribute_operators_and_case_sensitivity() {
    let sel_exact = Selector::parse("[data-role='admin']");
    let sel_prefix = Selector::parse("[href^='https://']");
    let sel_suffix = Selector::parse("[src$='.png']");
    let sel_substr = Selector::parse("[class*='btn-']");
    let sel_includes = Selector::parse("[rel~='nofollow']");
    let sel_dash = Selector::parse("[lang|='en']");
    let sel_case_i = Selector::parse("[title='example' i]");

    let mut attrs = HashMap::new();
    attrs.insert("data-role".to_string(), "admin".to_string());
    attrs.insert("href".to_string(), "https://github.com".to_string());
    attrs.insert("src".to_string(), "logo.png".to_string());
    attrs.insert("class".to_string(), "large btn-primary action".to_string());
    attrs.insert(
        "rel".to_string(),
        "noopener nofollow noreferrer".to_string(),
    );
    attrs.insert("lang".to_string(), "en-US".to_string());
    attrs.insert("title".to_string(), "EXAMPLE".to_string());

    assert!(sel_exact.matches("div", &[], None, &attrs));
    assert!(sel_prefix.matches("a", &[], None, &attrs));
    assert!(sel_suffix.matches("img", &[], None, &attrs));
    assert!(sel_substr.matches("button", &[], None, &attrs));
    assert!(sel_includes.matches("a", &[], None, &attrs));
    assert!(sel_dash.matches("html", &[], None, &attrs));
    assert!(sel_case_i.matches("span", &[], None, &attrs));
}

#[test]
fn test_pseudo_classes_evaluation() {
    let sel_not = Selector::parse("button:not([disabled])");
    let sel_is = Selector::parse(":is(h1, h2, h3)");
    let sel_nth = Selector::parse("li:nth-child(2n+1)");

    let enabled_attrs = HashMap::new();
    let enabled_ctx =
        ElementMatchingContext::simple("button", &[], None, &enabled_attrs, false, &[]);
    assert!(sel_not.matches_context(&enabled_ctx));

    let mut disabled_attrs = HashMap::new();
    disabled_attrs.insert("disabled".to_string(), "".to_string());
    let disabled_ctx =
        ElementMatchingContext::simple("button", &[], None, &disabled_attrs, false, &[]);
    assert!(!sel_not.matches_context(&disabled_ctx));

    let h2_ctx = ElementMatchingContext::simple("h2", &[], None, &enabled_attrs, false, &[]);
    assert!(sel_is.matches_context(&h2_ctx));

    let mut li_odd_ctx =
        ElementMatchingContext::simple("li", &[], None, &enabled_attrs, false, &[]);
    li_odd_ctx.index_in_parent = 3;
    assert!(sel_nth.matches_context(&li_odd_ctx));

    let mut li_even_ctx =
        ElementMatchingContext::simple("li", &[], None, &enabled_attrs, false, &[]);
    li_even_ctx.index_in_parent = 4;
    assert!(!sel_nth.matches_context(&li_even_ctx));
}

#[test]
fn test_combinators_child_and_descendant() {
    let sel_child = Selector::parse("nav > ul > li");
    let sel_desc = Selector::parse("article p");

    let ancestry = vec![
        ElementAncestry {
            tag: "ul".to_string(),
            classes: vec![],
            id: None,
        },
        ElementAncestry {
            tag: "nav".to_string(),
            classes: vec![],
            id: None,
        },
    ];
    let attrs = HashMap::new();
    let ctx = ElementMatchingContext::simple("li", &[], None, &attrs, false, &ancestry);
    assert!(sel_child.matches_context(&ctx));

    let deep_ancestry = vec![
        ElementAncestry {
            tag: "div".to_string(),
            classes: vec!["wrapper".to_string()],
            id: None,
        },
        ElementAncestry {
            tag: "section".to_string(),
            classes: vec![],
            id: None,
        },
        ElementAncestry {
            tag: "article".to_string(),
            classes: vec![],
            id: None,
        },
    ];
    let p_ctx = ElementMatchingContext::simple("p", &[], None, &attrs, false, &deep_ancestry);
    assert!(sel_desc.matches_context(&p_ctx));
}

#[test]
fn test_at_rules_media_supports_and_layers() {
    let css = r#"
        @layer base {
            p { color: #111111; }
        }
        @layer components {
            p { color: #222222; }
        }
        @media (max-width: 600px) {
            .mobile-only { display: block; }
        }
        @supports (display: flex) {
            .flex-container { display: flex; }
        }
    "#;

    let rules_desktop = parse_css_with_media(css, 1024);
    assert!(supports_query_matches("(display: flex)"));
    assert!(rules_desktop.iter().any(|r| r
        .declarations
        .iter()
        .any(|d| d.property == "display" && d.value == "flex")));
    assert!(!rules_desktop.iter().any(|r| r
        .selectors
        .iter()
        .any(|s| s.class.as_deref() == Some("mobile-only"))));

    let rules_mobile = parse_css_with_media(css, 500);
    assert!(rules_mobile.iter().any(|r| r
        .selectors
        .iter()
        .any(|s| s.class.as_deref() == Some("mobile-only"))));
}

#[test]
fn test_cascade_layers_and_importance() {
    let css = r#"
        @layer base {
            p { color: blue; }
        }
        @layer override {
            p { color: green; }
        }
    "#;

    let rules = parse_css(css);
    let style = compute_computed_style("p", &[], None, &rules, None, &HashMap::new());
    // Later layer (override) wins for normal declarations
    assert_eq!(style.color.as_deref(), Some("green"));

    // !important reverses layer priority in CSS Cascade Level 5
    let css_important = r#"
        @layer base {
            p { color: blue !important; }
        }
        @layer override {
            p { color: green !important; }
        }
    "#;
    let rules_imp = parse_css(css_important);
    let style_imp = compute_computed_style("p", &[], None, &rules_imp, None, &HashMap::new());
    assert_eq!(style_imp.color.as_deref(), Some("blue"));
}

#[test]
fn test_css_wide_keywords_and_inheritance() {
    let css = r#"
        .parent { color: red; font-size: 20px; margin: 10px; }
        .child-inherit { color: inherit; margin: inherit; }
        .child-initial { color: initial; }
        .child-unset { color: unset; margin: unset; }
    "#;
    let rules = parse_css(css);
    let parent_style = compute_computed_style(
        "div",
        &["parent".to_string()],
        None,
        &rules,
        None,
        &HashMap::new(),
    );
    assert_eq!(parent_style.color.as_deref(), Some("red"));
    assert_eq!(parent_style.font_size, Some(CssUnit::Pixels(20.0)));
    assert_eq!(parent_style.margin_top, Some(CssUnit::Pixels(10.0)));
    let child_style = compute_computed_style(
        "span",
        &[],
        None,
        &rules,
        Some(&parent_style),
        &HashMap::new(),
    );
    assert_eq!(child_style.color.as_deref(), Some("red"));
    assert_eq!(child_style.margin_top, None);
    let child_inherit = compute_computed_style(
        "div",
        &["child-inherit".to_string()],
        None,
        &rules,
        Some(&parent_style),
        &HashMap::new(),
    );
    assert_eq!(child_inherit.color.as_deref(), Some("red"));
    let child_init = compute_computed_style(
        "span",
        &["child-initial".to_string()],
        None,
        &rules,
        Some(&parent_style),
        &HashMap::new(),
    );
    assert_eq!(child_init.color.as_deref(), Some("#000000"));
}

#[test]
fn test_custom_properties_cycle_and_fallback() {
    let css = r#"
        :root {
            --primary: #3366cc;
            --accent: var(--primary);
            --fallback-test: var(--undefined-var, #ff5500);
            --cycle-a: var(--cycle-b);
            --cycle-b: var(--cycle-a);
        }
        .box {
            color: var(--accent);
            background-color: var(--fallback-test);
            border-color: var(--cycle-a, #00ff00);
        }
    "#;

    let rules = parse_css(css);
    let root_style = compute_computed_style("html", &[], None, &rules, None, &HashMap::new());
    let box_style = compute_computed_style(
        "div",
        &["box".to_string()],
        None,
        &rules,
        Some(&root_style),
        &HashMap::new(),
    );
    assert_eq!(box_style.color.as_deref(), Some("#3366cc"));
    assert_eq!(box_style.background_color.as_deref(), Some("#ff5500"));
    assert_eq!(box_style.border_color.as_deref(), Some("#00ff00"));
}

#[test]
fn test_math_functions_calc_min_max_clamp() {
    let customs = HashMap::new();
    let val_calc = eval_math_expression("calc(100% - 30px)", 500.0, 16.0, 1024.0, 768.0, &customs);
    assert_eq!(val_calc, Some(470.0));

    let val_min = eval_math_expression("min(50vw, 300px)", 500.0, 16.0, 1000.0, 768.0, &customs);
    assert_eq!(val_min, Some(300.0)); // 50vw = 500px, min(500, 300) = 300

    let val_max = eval_math_expression("max(10vw, 200px)", 500.0, 16.0, 1000.0, 768.0, &customs);
    assert_eq!(val_max, Some(200.0)); // 10vw = 100px, max(100, 200) = 200

    let val_clamp = eval_math_expression(
        "clamp(100px, 50vw, 400px)",
        500.0,
        16.0,
        1000.0,
        768.0,
        &customs,
    );
    assert_eq!(val_clamp, Some(400.0)); // 50vw = 500px clamped to [100, 400] = 400
}

#[test]
fn test_color_parsing_formats() {
    assert_eq!(parse_css_color("#123"), Some("#112233".to_string()));
    assert_eq!(parse_css_color("#1234"), Some("#11223344".to_string()));
    assert_eq!(parse_css_color("#112233"), Some("#112233".to_string()));
    assert_eq!(
        parse_css_color("rgb(0, 128, 255)"),
        Some("#0080ff".to_string())
    );
    assert_eq!(
        parse_css_color("rgba(0, 128, 255, 0.5)"),
        Some("#0080ff80".to_string())
    );
    assert_eq!(
        parse_css_color("hsl(120, 100%, 50%)"),
        Some("#00ff00".to_string())
    );
    assert_eq!(parse_css_color("royalblue"), Some("#4169e1".to_string()));
    assert_eq!(
        parse_css_color("transparent"),
        Some("transparent".to_string())
    );
    assert_eq!(
        parse_css_color("currentColor"),
        Some("currentColor".to_string())
    );
}

#[test]
fn test_diagnostics_recording() {
    reset_css_diagnostics();
    record_unsupported_property("backdrop-filter");
    record_unsupported_property("backdrop-filter");
    record_unsupported_property("scroll-timeline");

    let diag = get_css_diagnostics();
    assert_eq!(diag.unsupported_properties.get("backdrop-filter"), Some(&2));
    assert_eq!(diag.unsupported_properties.get("scroll-timeline"), Some(&1));
}

#[test]
fn test_wikipedia_sample_stylesheet() {
    let sample_css = include_str!("fixtures/css/wikipedia_sample.css");
    let rules = parse_css_with_media(sample_css, 1024);
    assert!(
        rules.len() > 10,
        "Should parse many rules from Wikipedia stylesheet"
    );

    let root_style = compute_computed_style("html", &[], None, &rules, None, &HashMap::new());
    assert!(root_style.custom_properties.contains_key("--color-base"));

    let mut attrs = HashMap::new();
    attrs.insert("href".to_string(), "/wiki/Rust".to_string());
    let link_style = compute_computed_style("a", &[], None, &rules, Some(&root_style), &attrs);
    assert_eq!(link_style.color.as_deref(), Some("#3366cc"));

    let button_style = compute_computed_style(
        "button",
        &[
            "cdx-button".to_string(),
            "cdx-button--action-progressive".to_string(),
        ],
        None,
        &rules,
        Some(&root_style),
        &HashMap::new(),
    );
    assert_eq!(button_style.background_color.as_deref(), Some("#3366cc"));
    assert_eq!(button_style.color.as_deref(), Some("#ffffff"));
}
