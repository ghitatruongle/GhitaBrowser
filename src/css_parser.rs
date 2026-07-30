// src/css_parser.rs - Real CSS Parser and Style Computation
#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq)]
pub struct CssRule {
    pub selector: String,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComputedStyle {
    pub color: Option<String>,
    pub font_family: Option<String>,
    pub background_color: Option<String>,
    pub display: Option<String>,
}

impl ComputedStyle {
    pub fn apply_declaration(&mut self, decl: &Declaration) {
        match decl.property.as_str() {
            "color" => self.color = Some(decl.value.clone()),
            "font-family" => self.font_family = Some(decl.value.clone()),
            "background-color" => self.background_color = Some(decl.value.clone()),
            "display" => self.display = Some(decl.value.clone()),
            _ => {}
        }
    }
}

/// Parse CSS string into rules
pub fn parse_inline_css(css: &str) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let trimmed = css.trim();
    if trimmed.is_empty() {
        return rules;
    }

    let mut pos = 0;
    let chars: Vec<char> = css.chars().collect();
    let len = chars.len();

    while pos < len {
        // Skip whitespace
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        if pos >= len {
            break;
        }

        // Read selector until '{'
        let sel_start = pos;
        while pos < len && chars[pos] != '{' {
            pos += 1;
        }
        if pos >= len {
            break;
        }
        let selector: String = chars[sel_start..pos].iter().collect();
        let selector = selector.trim().to_string();
        pos += 1; // skip '{'

        // Read declarations until '}'
        let mut declarations = Vec::new();
        while pos < len && chars[pos] != '}' {
            while pos < len && chars[pos].is_whitespace() {
                pos += 1;
            }
            if pos >= len || chars[pos] == '}' {
                break;
            }

            let prop_start = pos;
            while pos < len && chars[pos] != ':' && chars[pos] != '}' {
                pos += 1;
            }
            if pos >= len || chars[pos] == '}' {
                break;
            }
            let property: String = chars[prop_start..pos].iter().collect();
            let property = property.trim().to_lowercase();
            pos += 1; // skip ':'

            let val_start = pos;
            while pos < len && chars[pos] != ';' && chars[pos] != '}' {
                pos += 1;
            }
            let value: String = chars[val_start..pos].iter().collect();
            let value = value.trim().to_string();
            if pos < len && chars[pos] == ';' {
                pos += 1;
            }

            if !property.is_empty() && !value.is_empty() {
                declarations.push(Declaration { property, value });
            }
        }
        if pos < len && chars[pos] == '}' {
            pos += 1;
        }

        if !selector.is_empty() {
            rules.push(CssRule {
                selector,
                declarations,
            });
        }
    }

    rules
}

/// Compute final computed styles for an element based on CSS rules
pub fn compute_computed_style(
    element_tag: &str,
    rules: &[CssRule],
    parent_style: Option<&ComputedStyle>,
) -> ComputedStyle {
    let mut style = parent_style.cloned().unwrap_or_default();

    for rule in rules {
        if selector_matches(&rule.selector, element_tag) {
            for decl in &rule.declarations {
                style.apply_declaration(decl);
            }
        }
    }

    style
}

fn selector_matches(selector: &str, tag: &str) -> bool {
    let sel = selector.trim();
    sel == "*" || sel.eq_ignore_ascii_case(tag) || sel.starts_with('.') || sel.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_rule() {
        let css = "body { color: black; font-family: Arial; }";
        let rules = parse_inline_css(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selector, "body");
        assert_eq!(rules[0].declarations.len(), 2);
        assert_eq!(rules[0].declarations[0].property, "color");
        assert_eq!(rules[0].declarations[0].value, "black");
    }

    #[test]
    fn test_apply_declaration() {
        let mut style = ComputedStyle::default();
        let decl = Declaration {
            property: "color".to_string(),
            value: "red".to_string(),
        };
        style.apply_declaration(&decl);
        assert_eq!(style.color, Some("red".to_string()));
    }
}
