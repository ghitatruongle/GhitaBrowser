#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn new(tag: &str) -> Self {
        Element {
            tag: tag.to_string(),
            attrs: HashMap::new(),
            children: Vec::new(),
            text: String::new(),
        }
    }

    pub fn add_attr(&mut self, key: &str, value: &str) {
        self.attrs.insert(key.to_string(), value.to_string());
    }

    pub fn get_attr(&self, key: &str) -> Option<&String> {
        self.attrs.get(key)
    }

    pub fn add_child(&mut self, child: Element) {
        self.children.push(child);
    }

    /// Find element by tag recursively (first match)
    pub fn find_tag(&self, tag: &str) -> Option<&Element> {
        if self.tag == tag {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_tag(tag) {
                return Some(found);
            }
        }
        None
    }

    /// Find all elements matching tag recursively
    pub fn find_all_tags(&self, tag: &str) -> Vec<&Element> {
        let mut results = Vec::new();
        if self.tag == tag {
            results.push(self);
        }
        for child in &self.children {
            results.extend(child.find_all_tags(tag));
        }
        results
    }
}

/// Simple HTML parser that constructs a real DOM tree
pub fn parse_html(html: &str) -> Element {
    let html = html.trim();
    if html.is_empty() {
        return Element::new("body");
    }

    let mut stack: Vec<Element> = Vec::new();
    let root = Element::new("root");
    stack.push(root);

    let mut pos = 0;
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();

    while pos < len {
        if chars[pos] == '<' {
            if pos + 1 < len && chars[pos + 1] == '/' {
                // Closing tag: </tag>
                pos += 2;
                let start = pos;
                while pos < len && chars[pos] != '>' {
                    pos += 1;
                }
                let _close_tag: String = chars[start..pos].iter().collect();
                pos += 1; // skip '>'

                if stack.len() > 1 {
                    let completed = stack.pop().unwrap();
                    if let Some(parent) = stack.last_mut() {
                        parent.add_child(completed);
                    }
                }
            } else if pos + 1 < len && (chars[pos + 1] == '!' || chars[pos + 1] == '?') {
                // Comment or doctype: skip until '>'
                while pos < len && chars[pos] != '>' {
                    pos += 1;
                }
                if pos < len {
                    pos += 1;
                }
            } else {
                // Opening tag: <tag attr="val"> or <tag/>
                pos += 1; // skip '<'
                let start = pos;
                while pos < len && chars[pos] != '>' && chars[pos] != '/' && !chars[pos].is_whitespace() {
                    pos += 1;
                }
                let tag_name: String = chars[start..pos].iter().collect();
                let tag_name = tag_name.trim().to_lowercase();

                if tag_name.is_empty() {
                    continue;
                }

                let mut elem = Element::new(&tag_name);

                // Parse attributes
                while pos < len && chars[pos] != '>' && chars[pos] != '/' {
                    while pos < len && chars[pos].is_whitespace() {
                        pos += 1;
                    }
                    if pos >= len || chars[pos] == '>' || chars[pos] == '/' {
                        break;
                    }

                    let k_start = pos;
                    while pos < len && chars[pos] != '=' && chars[pos] != '>' && chars[pos] != '/' && !chars[pos].is_whitespace() {
                        pos += 1;
                    }
                    let key: String = chars[k_start..pos].iter().collect();

                    while pos < len && chars[pos].is_whitespace() {
                        pos += 1;
                    }

                    let mut val = String::new();
                    if pos < len && chars[pos] == '=' {
                        pos += 1; // skip '='
                        while pos < len && chars[pos].is_whitespace() {
                            pos += 1;
                        }
                        if pos < len && (chars[pos] == '"' || chars[pos] == '\'') {
                            let quote = chars[pos];
                            pos += 1;
                            let v_start = pos;
                            while pos < len && chars[pos] != quote {
                                pos += 1;
                            }
                            val = chars[v_start..pos].iter().collect();
                            if pos < len && chars[pos] == quote {
                                pos += 1;
                            }
                        } else {
                            let v_start = pos;
                            while pos < len && !chars[pos].is_whitespace() && chars[pos] != '>' && chars[pos] != '/' {
                                pos += 1;
                            }
                            val = chars[v_start..pos].iter().collect();
                        }
                    }
                    if !key.is_empty() {
                        elem.add_attr(&key, &val);
                    }
                }

                let is_self_closing = (pos < len && chars[pos] == '/') || is_void_tag(&tag_name);
                while pos < len && chars[pos] != '>' {
                    pos += 1;
                }
                if pos < len && chars[pos] == '>' {
                    pos += 1;
                }

                if is_self_closing {
                    if let Some(parent) = stack.last_mut() {
                        parent.add_child(elem);
                    }
                } else {
                    stack.push(elem);
                }
            }
        } else {
            // Text node
            let start = pos;
            while pos < len && chars[pos] != '<' {
                pos += 1;
            }
            let text: String = chars[start..pos].iter().collect();
            let trimmed_text = text.trim();
            if !trimmed_text.is_empty() {
                if let Some(current) = stack.last_mut() {
                    if current.text.is_empty() {
                        current.text = trimmed_text.to_string();
                    } else {
                        current.text.push(' ');
                        current.text.push_str(trimmed_text);
                    }
                }
            }
        }
    }

    while stack.len() > 1 {
        let completed = stack.pop().unwrap();
        if let Some(parent) = stack.last_mut() {
            parent.add_child(completed);
        }
    }

    let mut root = stack.pop().unwrap();
    if root.children.len() == 1 {
        root.children.remove(0)
    } else {
        root.tag = "html".to_string();
        root
    }
}

fn is_void_tag(tag: &str) -> bool {
    matches!(tag, "img" | "br" | "hr" | "input" | "meta" | "link")
}

pub fn fallback_parser(html: &str) -> Element {
    let mut elem = Element::new("div");
    let mut in_tag = false;
    let mut text = String::new();
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            text.push(c);
        }
    }
    elem.text = text.trim().to_string();
    elem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let html = "<html><body><h1>Hello</h1></body></html>";
        let dom = parse_html(html);
        assert!(dom.find_tag("h1").is_some());
        assert_eq!(dom.find_tag("h1").unwrap().text, "Hello");
    }
}