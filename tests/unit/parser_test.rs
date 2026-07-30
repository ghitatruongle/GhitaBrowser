// tests/unit/parser_test.rs - HTML parser tests
use ghitabrowser::parser::{Element, parse_html, fallback_parser};
use rstest::rstest;

#[rstest]
#[case("<html><title>Test</title></html>")]
#[case("<div><h1>Hello</h1><p>World</p></div>")]
#[case('<a href="https://example.com">Link</a>')]
#[case("<p class=\"test\">Paragraph</p>")]
fn test_parser_case(#[case] html: &str) {
    let element = parse_html(html);
    
    assert!(!element.tag.is_empty());
    assert!(!element.children.is_empty() || !element.text.is_empty());
    
    // Check basic structure
    if let Some(title_elem) = element.find_tag("title") {
        assert!(!title_elem.text.is_empty());
    }
}

#[test]
fn test_fallback_parser() {
    let html = "<div>This is test <b>bold</b> text.</div>";
    let element = fallback_parser(html);
    
    assert!(!element.text.is_empty());
    assert!(element.text.contains("test"));
    assert!(!element.text.contains("<")); // Tags should be removed
}

#[rstest]
#[case("h1")]
#[case("p")]
#[case("a")]
#[case("img")]
fn test_find_tag(#[case] tag: &str) {
    let html = format!("<div><{tag}>Content</{tag}></div>", tag = tag);
    let element = parse_html(&html);
    
    let found = element.find_tag(tag);
    assert!(found.is_some(), "Should find element of tag {}", tag);
    assert_eq!(found.unwrap().tag, tag.to_string());
}

#[rstest]
#[case('<a href="https://google.com">Google</a>', "Google", "https://google.com")]
#[case('<a href="https://github.com">GitHub</a>', "GitHub", "https://github.com")]
fn test_anchor_element(#[case] html: &str, #[case] expected_text: &str, #[case] expected_href: &str) {
    let element = parse_html(html);
    let anchors = element.find_all_tags("a");
    
    assert_eq!(anchors.len(), 1);
    let anchor = anchors[0];
    
    assert_eq!(anchor.text, expected_text);
    assert_eq!(anchor.get_attr("href"), Some(&expected_href.to_string()));
}