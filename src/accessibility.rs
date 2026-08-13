//! Bounded semantic accessibility tree derived from the parsed DOM.

use crate::parser::Element;

const MAX_ACCESSIBLE_NODES: usize = 20_000;
const MAX_ACCESSIBLE_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AccessibleRole {
    Document,
    Banner,
    Navigation,
    Main,
    ContentInfo,
    Heading,
    Link,
    Button,
    TextBox,
    CheckBox,
    Radio,
    ComboBox,
    Image,
    List,
    ListItem,
    Table,
    Row,
    Cell,
    Paragraph,
    Form,
    ListBox,
    ListOption,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessibleNode {
    pub role: AccessibleRole,
    pub name: String,
    pub value: Option<String>,
    pub disabled: bool,
    pub checked: Option<bool>,
    pub level: Option<u8>,
    /// `required` constraint state (form controls, Phase 21).
    pub required: bool,
    pub children: Vec<AccessibleNode>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessibilityTree {
    pub root: Option<AccessibleNode>,
    pub node_count: usize,
    pub truncated: bool,
}

pub fn build_tree(root: &Element) -> AccessibilityTree {
    let mut budget = Budget {
        node_count: 0,
        truncated: false,
    };
    let accessible_root = build_node(root, 0, &mut budget);
    AccessibilityTree {
        root: accessible_root,
        node_count: budget.node_count,
        truncated: budget.truncated,
    }
}

struct Budget {
    node_count: usize,
    truncated: bool,
}

fn build_node(element: &Element, depth: usize, budget: &mut Budget) -> Option<AccessibleNode> {
    if depth > MAX_ACCESSIBLE_DEPTH || budget.node_count >= MAX_ACCESSIBLE_NODES {
        budget.truncated = true;
        return None;
    }
    if is_hidden(element) {
        return None;
    }

    budget.node_count += 1;
    let mut children = Vec::new();
    for child in &element.children {
        if let Some(node) = build_node(child, depth + 1, budget) {
            children.push(node);
        }
        if budget.node_count >= MAX_ACCESSIBLE_NODES {
            budget.truncated = true;
            break;
        }
    }

    let role = explicit_role(element).unwrap_or_else(|| {
        role_for_tag(&element.tag, element.get_attr("type").map(String::as_str))
    });
    let name = accessible_name(element);
    // Textarea content lives in its text children; inputs use the value
    // attribute. Selects report the selected option when one is marked.
    let value = match role {
        AccessibleRole::TextBox if element.tag == "textarea" => {
            let content = accessible_text(element);
            (!content.is_empty()).then_some(content)
        }
        AccessibleRole::ComboBox => element
            .children
            .iter()
            .find(|child| child.attrs.contains_key("selected"))
            .map(|option| {
                option
                    .get_attr("value")
                    .cloned()
                    .unwrap_or_else(|| accessible_text(option))
            })
            .or_else(|| element.get_attr("value").cloned()),
        AccessibleRole::TextBox => element.get_attr("value").cloned(),
        _ => None,
    };
    let checked = matches!(role, AccessibleRole::CheckBox | AccessibleRole::Radio).then(|| {
        element.attrs.contains_key("checked")
            || element.get_attr("aria-checked").map(String::as_str) == Some("true")
    });
    let level = if role == AccessibleRole::Heading {
        element
            .tag
            .strip_prefix('h')
            .and_then(|level| level.parse::<u8>().ok())
            .or_else(|| {
                element
                    .get_attr("aria-level")
                    .and_then(|level| level.parse::<u8>().ok())
            })
    } else {
        None
    };

    Some(AccessibleNode {
        role,
        name,
        value,
        disabled: element.attrs.contains_key("disabled")
            || element.get_attr("aria-disabled").map(String::as_str) == Some("true"),
        checked,
        level,
        required: element.attrs.contains_key("required")
            || element.get_attr("aria-required").map(String::as_str) == Some("true"),
        children,
    })
}

fn accessible_name(element: &Element) -> String {
    for attribute in ["aria-label", "alt", "title", "placeholder", "value"] {
        if let Some(value) = element.get_attr(attribute) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.chars().take(512).collect();
            }
        }
    }
    accessible_text(element)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}

fn accessible_text(element: &Element) -> String {
    if is_hidden(element) {
        return String::new();
    }

    let mut text = element.text.clone();
    for child in &element.children {
        text.push(' ');
        text.push_str(&accessible_text(child));
    }
    text
}

fn is_hidden(element: &Element) -> bool {
    element.get_attr("aria-hidden").map(String::as_str) == Some("true")
        || element.attrs.contains_key("hidden")
        || element
            .get_attr("style")
            .is_some_and(|style| style.to_ascii_lowercase().contains("display:none"))
}

fn explicit_role(element: &Element) -> Option<AccessibleRole> {
    Some(match element.get_attr("role")?.as_str() {
        "banner" => AccessibleRole::Banner,
        "navigation" => AccessibleRole::Navigation,
        "main" => AccessibleRole::Main,
        "contentinfo" => AccessibleRole::ContentInfo,
        "heading" => AccessibleRole::Heading,
        "link" => AccessibleRole::Link,
        "button" => AccessibleRole::Button,
        "textbox" => AccessibleRole::TextBox,
        "checkbox" => AccessibleRole::CheckBox,
        "radio" => AccessibleRole::Radio,
        "combobox" => AccessibleRole::ComboBox,
        "img" => AccessibleRole::Image,
        "list" => AccessibleRole::List,
        "listitem" => AccessibleRole::ListItem,
        "form" => AccessibleRole::Form,
        "listbox" => AccessibleRole::ListBox,
        "option" => AccessibleRole::ListOption,
        "table" => AccessibleRole::Table,
        "row" => AccessibleRole::Row,
        "cell" => AccessibleRole::Cell,
        _ => AccessibleRole::Generic,
    })
}

fn role_for_tag(tag: &str, input_type: Option<&str>) -> AccessibleRole {
    match tag {
        "html" | "body" => AccessibleRole::Document,
        "header" => AccessibleRole::Banner,
        "nav" => AccessibleRole::Navigation,
        "main" => AccessibleRole::Main,
        "footer" => AccessibleRole::ContentInfo,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => AccessibleRole::Heading,
        "a" => AccessibleRole::Link,
        "button" => AccessibleRole::Button,
        "textarea" => AccessibleRole::TextBox,
        "select" => AccessibleRole::ComboBox,
        "input" => match input_type.unwrap_or("text") {
            "checkbox" => AccessibleRole::CheckBox,
            "radio" => AccessibleRole::Radio,
            "button" | "submit" | "reset" => AccessibleRole::Button,
            _ => AccessibleRole::TextBox,
        },
        "img" => AccessibleRole::Image,
        "ul" | "ol" => AccessibleRole::List,
        "li" => AccessibleRole::ListItem,
        "form" => AccessibleRole::Form,
        "option" => AccessibleRole::ListOption,
        "table" => AccessibleRole::Table,
        "tr" => AccessibleRole::Row,
        "td" | "th" => AccessibleRole::Cell,
        "p" => AccessibleRole::Paragraph,
        _ => AccessibleRole::Generic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_roles_names_and_states() {
        let dom = crate::parser::parse_html(
            "<main><h2>Account</h2><img alt='Avatar'><input type='checkbox' aria-label='Remember' checked><button disabled>Save</button></main>",
        );
        let tree = build_tree(&dom);
        let root = tree.root.unwrap();
        let debug = format!("{root:?}");
        assert!(debug.contains("Heading"));
        assert!(debug.contains("Avatar"));
        assert!(debug.contains("Remember"));
        assert!(debug.contains("checked: Some(true)"));
        assert!(debug.contains("disabled: true"));
    }

    #[test]
    fn hidden_subtrees_are_excluded() {
        let dom = crate::parser::parse_html(
            "<main><p aria-hidden='true'>secret</p><p>visible</p></main>",
        );
        let debug = format!("{:?}", build_tree(&dom));
        assert!(!debug.contains("secret"));
        assert!(debug.contains("visible"));
    }
}
