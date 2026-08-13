//! Minimal mutable DOM operations used by the independent web runtime.

use crate::parser::Element;

pub fn get_element_by_id_mut<'a>(root: &'a mut Element, id: &str) -> Option<&'a mut Element> {
    if root.get_attr("id").map(String::as_str) == Some(id) {
        return Some(root);
    }
    root.children
        .iter_mut()
        .find_map(|child| get_element_by_id_mut(child, id))
}

pub fn get_element_by_id<'a>(root: &'a Element, id: &str) -> Option<&'a Element> {
    if root.get_attr("id").map(String::as_str) == Some(id) {
        return Some(root);
    }
    root.children
        .iter()
        .find_map(|child| get_element_by_id(child, id))
}

pub fn query_selector_mut<'a>(root: &'a mut Element, selector: &str) -> Option<&'a mut Element> {
    if matches_selector(root, selector) {
        return Some(root);
    }
    root.children
        .iter_mut()
        .find_map(|child| query_selector_mut(child, selector))
}

pub fn query_selector<'a>(root: &'a Element, selector: &str) -> Option<&'a Element> {
    if matches_selector(root, selector) {
        return Some(root);
    }
    root.children
        .iter()
        .find_map(|child| query_selector(child, selector))
}

pub fn set_text_content(element: &mut Element, value: &str) {
    element.children.clear();
    element.text = value.chars().take(1024 * 1024).collect();
}

pub fn set_attribute(element: &mut Element, name: &str, value: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 128
        || name.starts_with("on")
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_:".contains(character))
    {
        return false;
    }
    element
        .attrs
        .insert(name, value.chars().take(64 * 1024).collect());
    true
}

fn matches_selector(element: &Element, selector: &str) -> bool {
    let selector = selector.trim();
    if let Some(id) = selector.strip_prefix('#') {
        return element.get_attr("id").map(String::as_str) == Some(id);
    }
    if let Some(class) = selector.strip_prefix('.') {
        return element
            .get_attr("class")
            .is_some_and(|classes| classes.split_ascii_whitespace().any(|value| value == class));
    }
    !selector.is_empty() && element.tag.eq_ignore_ascii_case(selector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_and_mutates_document() {
        let mut dom = crate::parser::parse_html("<main><p id='message'>old</p></main>");
        let target = get_element_by_id_mut(&mut dom, "message").unwrap();
        set_text_content(target, "new");
        assert!(set_attribute(target, "aria-live", "polite"));
        assert!(!set_attribute(target, "onclick", "bad()"));
        assert_eq!(target.text, "new");
        assert_eq!(
            target.get_attr("aria-live").map(String::as_str),
            Some("polite")
        );
    }
}
