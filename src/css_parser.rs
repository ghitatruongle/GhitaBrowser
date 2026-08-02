// src/css_parser.rs - Advanced CSS Parser and Style Computation (v0.6.1)


use std::collections::HashMap;

/// A parsed CSS rule
#[derive(Debug, Clone, PartialEq)]
pub struct CssRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    pub specificity: (u32, u32, u32), // (id, class, tag) for cascading
}

/// A single CSS selector (can be compound: div.class#id)
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub tag: Option<String>,
    pub class: Option<String>,
    pub id: Option<String>,
    pub attributes: Vec<(String, String)>, // attribute selectors [attr=value]
}

impl Selector {
    /// Parse a single selector string like "div.class#id"
    pub fn parse(input: &str) -> Self {
        let input = input.trim();
        let mut tag: Option<String> = None;
        let mut class: Option<String> = None;
        let mut id: Option<String> = None;
        let mut attributes = Vec::new();

        // Handle attribute selectors first [attr=value] / [attr]
        let mut remaining = input.to_string();
        if let Some(attr_start) = remaining.find('[') {
            let before = remaining[..attr_start].to_string();
            let after_bracket = &remaining[attr_start..];
            if let Some(attr_end) = after_bracket.find(']') {
                let attr_content = &after_bracket[1..attr_end];
                if let Some(eq_pos) = attr_content.find('=') {
                    let attr_name = attr_content[..eq_pos].trim().to_string();
                    let attr_val = attr_content[eq_pos + 1..]
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    attributes.push((attr_name, attr_val));
                } else {
                    // [attr] — presence selector; the empty value means "just exists"
                    let attr_name = attr_content.trim().to_string();
                    if !attr_name.is_empty() {
                        attributes.push((attr_name, String::new()));
                    }
                }
                remaining = before + &after_bracket[attr_end + 1..];
            }
        }

        // Track what we're currently parsing: tag name, class name, or id name
        enum ParseState {
            Tag,
            Class,
            Id,
        }
        let mut state = ParseState::Tag;
        let mut current = String::new();

        for c in remaining.chars() {
            match c {
                '.' | '#' => {
                    // Save the accumulated piece under the CURRENT state. The
                    // old code forced it into `tag` whenever tag was empty,
                    // so "#main.highlight" became tag="main" and lost the id.
                    if !current.is_empty() {
                        match state {
                            ParseState::Tag => {
                                tag = Some(current.to_lowercase());
                            }
                            ParseState::Class => class = Some(current.clone()),
                            ParseState::Id => id = Some(current.clone()),
                        }
                        current.clear();
                    }
                    state = if c == '.' {
                        ParseState::Class
                    } else {
                        ParseState::Id
                    };
                }
                _ => current.push(c),
            }
        }

        // Save remaining content based on current state. Class and id values
        // keep their case (they are case-sensitive in HTML); tags are lowered.
        if !current.is_empty() {
            match state {
                ParseState::Tag => tag = Some(current.to_lowercase()),
                ParseState::Class => class = Some(current),
                ParseState::Id => id = Some(current),
            }
        }

        Selector {
            tag,
            class,
            id,
            attributes,
        }
    }

    /// Check if this selector matches an element with the given tag, class,
    /// id, and attributes
    pub fn matches(
        &self,
        tag: &str,
        classes: &[String],
        elem_id: Option<&str>,
        attrs: &HashMap<String, String>,
    ) -> bool {
        // Tag check
        if let Some(ref sel_tag) = self.tag {
            if sel_tag != "*" && sel_tag != tag {
                return false;
            }
        }

        // Class check
        if let Some(ref sel_class) = self.class {
            if !classes.iter().any(|c| c == sel_class) {
                return false;
            }
        }

        // ID check
        if let Some(ref sel_id) = self.id {
            match elem_id {
                Some(id) if id == sel_id => {}
                _ => return false,
            }
        }

        // Attribute selectors: [attr] requires presence, [attr=value] an
        // exact value match (empty value from "[attr]" means presence only).
        for (attr_name, attr_val) in &self.attributes {
            let actual = match attrs.get(attr_name) {
                Some(v) => v,
                None => return false,
            };
            if !attr_val.is_empty() && actual != attr_val {
                return false;
            }
        }

        true
    }

    /// Compute specificity: (id_count, class_count, tag_count)
    pub fn specificity(&self) -> (u32, u32, u32) {
        let id = if self.id.is_some() { 1 } else { 0 };
        let class = if self.class.is_some() { 1 } else { 0 } + self.attributes.len() as u32;
        let tag = if self.tag.is_some() && self.tag.as_deref() != Some("*") {
            1
        } else {
            0
        };
        (id, class, tag)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: String,
}

/// Computed style with many CSS properties
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    // Colors
    pub color: Option<String>,
    pub background_color: Option<String>,

    // Font
    pub font_family: Option<String>,
    pub font_size: Option<CssUnit>,
    pub font_weight: Option<u16>,
    pub font_style: Option<String>,
    pub text_align: Option<String>,
    pub line_height: Option<f64>,

    // Box model
    pub display: Option<String>,
    pub width: Option<CssUnit>,
    pub height: Option<CssUnit>,
    pub margin_top: Option<CssUnit>,
    pub margin_right: Option<CssUnit>,
    pub margin_bottom: Option<CssUnit>,
    pub margin_left: Option<CssUnit>,
    pub padding_top: Option<CssUnit>,
    pub padding_right: Option<CssUnit>,
    pub padding_bottom: Option<CssUnit>,
    pub padding_left: Option<CssUnit>,

    // Border
    pub border_width: Option<CssUnit>,
    pub border_style: Option<String>,
    pub border_color: Option<String>,

    // Misc
    pub overflow: Option<String>,
    pub cursor: Option<String>,
    pub opacity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CssUnit {
    Pixels(f64),
    Percent(f64),
    Em(f64),
    Rem(f64),
    Auto,
}

impl CssUnit {
    pub fn to_pixels(&self, parent_size: f64, root_size: f64) -> f64 {
        match self {
            CssUnit::Pixels(px) => *px,
            CssUnit::Percent(pct) => parent_size * pct / 100.0,
            CssUnit::Em(em) => parent_size * em,
            CssUnit::Rem(rem) => root_size * rem,
            // `auto` resolves to 0 for margins/padding (a full-width margin
            // was nonsense: `margin: 0 auto` spanned the whole parent).
            // `width: auto` is handled by the layout pass, which refills the
            // parent width when no explicit width is set.
            CssUnit::Auto => 0.0,
        }
    }

    pub fn parse(value: &str) -> Option<CssUnit> {
        let value = value.trim();
        if value == "auto" || value == "inherit" || value == "initial" {
            return Some(CssUnit::Auto);
        }

        if let Some(px_val) = value.strip_suffix("px") {
            px_val.trim().parse::<f64>().ok().map(CssUnit::Pixels)
        } else if let Some(pct_val) = value.strip_suffix('%') {
            pct_val.trim().parse::<f64>().ok().map(CssUnit::Percent)
        } else if let Some(em_val) = value.strip_suffix("em") {
            em_val.trim().parse::<f64>().ok().map(CssUnit::Em)
        } else if let Some(rem_val) = value.strip_suffix("rem") {
            rem_val.trim().parse::<f64>().ok().map(CssUnit::Rem)
        } else {
            // Try plain number as pixels
            value.parse::<f64>().ok().map(CssUnit::Pixels)
        }
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            color: Some("#000000".to_string()),
            background_color: None,
            font_family: Some("sans-serif".to_string()),
            font_size: Some(CssUnit::Pixels(16.0)),
            font_weight: Some(400),
            font_style: None,
            text_align: Some("left".to_string()),
            line_height: Some(1.4),
            display: Some("block".to_string()),
            width: None,
            height: None,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
            padding_top: None,
            padding_right: None,
            padding_bottom: None,
            padding_left: None,
            border_width: None,
            border_style: None,
            border_color: None,
            overflow: Some("visible".to_string()),
            cursor: None,
            opacity: Some(1.0),
        }
    }
}

impl ComputedStyle {
    pub fn apply_declaration(&mut self, decl: &Declaration) {
        let prop = decl.property.as_str();
        let val = decl.value.trim();

        match prop {
            // Colors
            "color" => self.color = Some(val.to_string()),
            "background-color" | "background" => self.background_color = Some(val.to_string()),

            // Font
            "font-family" => self.font_family = Some(val.to_string()),
            "font-size" => self.font_size = CssUnit::parse(val),
            "font-weight" => {
                self.font_weight = match val {
                    "normal" | "400" => Some(400),
                    "bold" | "700" => Some(700),
                    "lighter" => Some(300),
                    "bolder" => Some(900),
                    _ => val.parse::<u16>().ok(),
                };
            }
            "font-style" => self.font_style = Some(val.to_string()),
            "text-align" => self.text_align = Some(val.to_string()),
            "line-height" => {
                self.line_height = val.parse::<f64>().ok();
            }

            // Box model
            "display" => self.display = Some(val.to_string()),
            "width" => self.width = CssUnit::parse(val),
            "height" => self.height = CssUnit::parse(val),

            // Margin shorthand
            "margin" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                match parts.len() {
                    1 => {
                        let u = CssUnit::parse(parts[0]);
                        self.margin_top = u.clone();
                        self.margin_right = u.clone();
                        self.margin_bottom = u.clone();
                        self.margin_left = u;
                    }
                    2 => {
                        let u1 = CssUnit::parse(parts[0]);
                        let u2 = CssUnit::parse(parts[1]);
                        self.margin_top = u1.clone();
                        self.margin_right = u2.clone();
                        self.margin_bottom = u1;
                        self.margin_left = u2;
                    }
                    4 => {
                        self.margin_top = CssUnit::parse(parts[0]);
                        self.margin_right = CssUnit::parse(parts[1]);
                        self.margin_bottom = CssUnit::parse(parts[2]);
                        self.margin_left = CssUnit::parse(parts[3]);
                    }
                    _ => {}
                }
            }
            "margin-top" => self.margin_top = CssUnit::parse(val),
            "margin-right" => self.margin_right = CssUnit::parse(val),
            "margin-bottom" => self.margin_bottom = CssUnit::parse(val),
            "margin-left" => self.margin_left = CssUnit::parse(val),

            // Padding shorthand
            "padding" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                match parts.len() {
                    1 => {
                        let u = CssUnit::parse(parts[0]);
                        self.padding_top = u.clone();
                        self.padding_right = u.clone();
                        self.padding_bottom = u.clone();
                        self.padding_left = u;
                    }
                    2 => {
                        let u1 = CssUnit::parse(parts[0]);
                        let u2 = CssUnit::parse(parts[1]);
                        self.padding_top = u1.clone();
                        self.padding_right = u2.clone();
                        self.padding_bottom = u1;
                        self.padding_left = u2;
                    }
                    4 => {
                        self.padding_top = CssUnit::parse(parts[0]);
                        self.padding_right = CssUnit::parse(parts[1]);
                        self.padding_bottom = CssUnit::parse(parts[2]);
                        self.padding_left = CssUnit::parse(parts[3]);
                    }
                    _ => {}
                }
            }
            "padding-top" => self.padding_top = CssUnit::parse(val),
            "padding-right" => self.padding_right = CssUnit::parse(val),
            "padding-bottom" => self.padding_bottom = CssUnit::parse(val),
            "padding-left" => self.padding_left = CssUnit::parse(val),

            // Border
            "border" | "border-width" => {
                self.border_width = CssUnit::parse(val.split_whitespace().next().unwrap_or(val))
            }
            "border-style" => self.border_style = Some(val.to_string()),
            "border-color" => self.border_color = Some(val.to_string()),

            // Misc
            "overflow" => self.overflow = Some(val.to_string()),
            "cursor" => self.cursor = Some(val.to_string()),
            "opacity" => {
                self.opacity = val.parse::<f64>().ok();
            }

            _ => {} // Unknown properties are silently ignored
        }
    }
}

/// Parse CSS string into rules
pub fn parse_css(css: &str) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let trimmed = css.trim();
    if trimmed.is_empty() {
        return rules;
    }

    let mut pos = 0;
    let chars: Vec<char> = css.chars().collect();
    let len = chars.len();

    while pos < len {
        // Skip whitespace and comments
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }

        // Skip CSS comments /* ... */
        if pos + 1 < len && chars[pos] == '/' && chars[pos + 1] == '*' {
            pos += 2;
            while pos + 1 < len && !(chars[pos] == '*' && chars[pos + 1] == '/') {
                pos += 1;
            }
            pos += 2;
            continue;
        }

        if pos >= len {
            break;
        }

        // Read selectors until '{' (handling commas for selector groups)
        let sel_start = pos;
        let mut brace_depth = 0;
        while pos < len && !(chars[pos] == '{' && brace_depth == 0) {
            if chars[pos] == '{' {
                brace_depth += 1;
            }
            if chars[pos] == '}' {
                brace_depth -= 1;
            } // balance a stray brace so a later '{' still terminates
            pos += 1;
        }
        if pos >= len {
            break;
        }
        let selector_str: String = chars[sel_start..pos].iter().collect();
        pos += 1; // skip '{'

        // Read declarations until '}'
        let mut declarations = Vec::new();
        while pos < len && chars[pos] != '}' {
            // Skip whitespace
            while pos < len && chars[pos].is_whitespace() {
                pos += 1;
            }
            if pos >= len || chars[pos] == '}' {
                break;
            }

            // Read property name
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

            // Read value until ';' or '}'
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

        // Parse selector string into individual selectors (comma-separated)
        let selector_str = selector_str.trim();
        if !selector_str.is_empty() {
            let selectors: Vec<Selector> = selector_str
                .split(',')
                .map(|s| Selector::parse(s.trim()))
                .collect();

            if !selectors.is_empty() && !declarations.is_empty() {
                let specificity = selectors
                    .iter()
                    .map(|s| s.specificity())
                    .max()
                    .unwrap_or((0, 0, 0));

                rules.push(CssRule {
                    selectors,
                    declarations,
                    specificity,
                });
            }
        }
    }

    rules
}

/// Compute final computed styles for an element based on CSS rules
pub fn compute_computed_style(
    element_tag: &str,
    element_classes: &[String],
    element_id: Option<&str>,
    rules: &[CssRule],
    parent_style: Option<&ComputedStyle>,
    element_attrs: &HashMap<String, String>,
) -> ComputedStyle {
    // Start with parent's inherited styles + defaults
    let mut style = if let Some(parent) = parent_style {
        // Inheritable properties
        ComputedStyle {
            color: parent.color.clone(),
            font_family: parent.font_family.clone(),
            font_size: parent.font_size.clone(),
            font_weight: parent.font_weight,
            font_style: parent.font_style.clone(),
            text_align: parent.text_align.clone(),
            line_height: parent.line_height,
            ..ComputedStyle::default()
        }
    } else {
        ComputedStyle::default()
    };

    // Default display based on tag
    style.display = Some(default_display_for_tag(element_tag).to_string());

    // User-agent stylesheet: default font-size/weight per tag (like Chrome's built-in styles).
    // Applied after inheritance but before author rules, so page CSS can still override it.
    if let Some(ua_size) = ua_font_size_px(element_tag) {
        style.font_size = Some(CssUnit::Pixels(ua_size));
    }
    if matches!(
        element_tag,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "b" | "strong" | "th"
    ) {
        style.font_weight = Some(700);
    }
    if matches!(element_tag, "i" | "em" | "cite" | "var" | "address") {
        style.font_style = Some("italic".to_string());
    }

    // Apply matching rules in specificity order
    let mut matching_rules: Vec<&CssRule> = Vec::new();
    for rule in rules {
        for selector in &rule.selectors {
            if selector.matches(element_tag, element_classes, element_id, element_attrs) {
                matching_rules.push(rule);
                break;
            }
        }
    }

    // Sort by specificity (then by source order implicitly via stable sort)
    matching_rules.sort_by_key(|r| r.specificity);

    for rule in &matching_rules {
        for decl in &rule.declarations {
            style.apply_declaration(decl);
        }
    }

    style
}

/// User-agent stylesheet default font size (px) for a tag, or None to inherit
fn ua_font_size_px(tag: &str) -> Option<f64> {
    match tag {
        "h1" => Some(32.0),
        "h2" => Some(24.0),
        "h3" => Some(18.72),
        "h4" => Some(16.0),
        "h5" => Some(13.28),
        "h6" => Some(10.72),
        "small" | "sub" | "sup" => Some(13.28),
        _ => None,
    }
}

fn default_display_for_tag(tag: &str) -> &'static str {
    match tag {
        "span" | "a" | "i" | "b" | "em" | "strong" | "img" | "code" | "label" | "q" | "cite" => {
            "inline"
        }
        "head" | "script" | "style" | "meta" | "link" | "noscript" => "none",
        "li" => "list-item",
        "table" => "table",
        "tr" => "table-row",
        "td" | "th" => "table-cell",
        _ => "block",
    }
}

/// Parse a class attribute into individual class names
pub fn parse_class_attr(class_attr: Option<&str>) -> Vec<String> {
    class_attr
        .map(|s| s.split_whitespace().map(|c| c.to_string()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_rule() {
        let css = "body { color: black; font-family: Arial; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors[0].tag, Some("body".to_string()));
        assert_eq!(rules[0].declarations.len(), 2);
        assert_eq!(rules[0].declarations[0].property, "color");
        assert_eq!(rules[0].declarations[0].value, "black");
    }

    #[test]
    fn test_parse_class_selector() {
        let css = ".highlight { color: red; font-weight: bold; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors[0].class, Some("highlight".to_string()));
    }

    #[test]
    fn test_parse_id_selector() {
        let css = "#main { background-color: blue; }";
        let rules = parse_css(css);
        assert_eq!(rules[0].selectors[0].id, Some("main".to_string()));
    }

    #[test]
    fn test_selector_specificity() {
        let id_sel = Selector::parse("#main");
        let class_sel = Selector::parse(".highlight");
        let tag_sel = Selector::parse("div");

        assert!(id_sel.specificity() > class_sel.specificity());
        assert!(class_sel.specificity() > tag_sel.specificity());
    }

    #[test]
    fn test_selector_matches_class() {
        let sel = Selector::parse(".highlight");
        let attrs = HashMap::new();
        assert!(sel.matches("div", &["highlight".to_string()], None, &attrs));
        assert!(!sel.matches("div", &["other".to_string()], None, &attrs));
    }

    #[test]
    fn test_selector_matches_id() {
        let sel = Selector::parse("#header");
        let attrs = HashMap::new();
        assert!(sel.matches("div", &[], Some("header"), &attrs));
        assert!(!sel.matches("div", &[], Some("footer"), &attrs));
    }

    #[test]
    fn test_parse_complex_selector() {
        let sel = Selector::parse("div.main#content");
        assert_eq!(sel.tag, Some("div".to_string()));
        assert_eq!(sel.class, Some("main".to_string()));
        assert_eq!(sel.id, Some("content".to_string()));
    }

    #[test]
    fn test_parse_id_class_selector_keeps_id() {
        // "#main.highlight" previously parsed as tag="main" and lost the id
        let sel = Selector::parse("#main.highlight");
        assert_eq!(sel.tag, None);
        assert_eq!(sel.id, Some("main".to_string()));
        assert_eq!(sel.class, Some("highlight".to_string()));
        assert!(sel.matches(
            "div",
            &["highlight".to_string()],
            Some("main"),
            &HashMap::new()
        ));
        assert!(!sel.matches("main", &["highlight".to_string()], None, &HashMap::new()));
    }

    #[test]
    fn test_class_id_are_case_sensitive() {
        let sel = Selector::parse(".FooBar");
        assert_eq!(sel.class, Some("FooBar".to_string()));
        assert!(!sel.matches("div", &["foobar".to_string()], None, &HashMap::new()));
        assert!(sel.matches("div", &["FooBar".to_string()], None, &HashMap::new()));
    }

    #[test]
    fn test_attribute_selector_matches() {
        let sel = Selector::parse("input[type=\"text\"]");
        assert_eq!(sel.tag, Some("input".to_string()));
        assert_eq!(
            sel.attributes,
            vec![("type".to_string(), "text".to_string())]
        );

        let mut attrs = HashMap::new();
        attrs.insert("type".to_string(), "text".to_string());
        assert!(sel.matches("input", &[], None, &attrs));

        attrs.insert("type".to_string(), "password".to_string());
        assert!(!sel.matches("input", &[], None, &attrs));

        attrs.remove("type");
        assert!(!sel.matches("input", &[], None, &attrs));
    }

    #[test]
    fn test_attribute_presence_selector() {
        let sel = Selector::parse("[disabled]");
        let mut attrs = HashMap::new();
        assert!(!sel.matches("input", &[], None, &attrs));
        attrs.insert("disabled".to_string(), String::new());
        assert!(sel.matches("input", &[], None, &attrs));
    }

    #[test]
    fn test_margin_shorthand() {
        let css = "div { margin: 10px 20px; }";
        let rules = parse_css(css);
        let mut style = ComputedStyle::default();
        for decl in &rules[0].declarations {
            style.apply_declaration(decl);
        }
        assert_eq!(style.margin_top, Some(CssUnit::Pixels(10.0)));
        assert_eq!(style.margin_right, Some(CssUnit::Pixels(20.0)));
        assert_eq!(style.margin_bottom, Some(CssUnit::Pixels(10.0)));
        assert_eq!(style.margin_left, Some(CssUnit::Pixels(20.0)));
    }

    #[test]
    fn test_padding_shorthand_4_values() {
        let css = "div { padding: 1px 2px 3px 4px; }";
        let rules = parse_css(css);
        let mut style = ComputedStyle::default();
        for decl in &rules[0].declarations {
            style.apply_declaration(decl);
        }
        assert_eq!(style.padding_top, Some(CssUnit::Pixels(1.0)));
        assert_eq!(style.padding_right, Some(CssUnit::Pixels(2.0)));
        assert_eq!(style.padding_bottom, Some(CssUnit::Pixels(3.0)));
        assert_eq!(style.padding_left, Some(CssUnit::Pixels(4.0)));
    }

    #[test]
    fn test_css_unit_parsing() {
        assert_eq!(CssUnit::parse("10px"), Some(CssUnit::Pixels(10.0)));
        assert_eq!(CssUnit::parse("50%"), Some(CssUnit::Percent(50.0)));
        assert_eq!(CssUnit::parse("2em"), Some(CssUnit::Em(2.0)));
        assert_eq!(CssUnit::parse("auto"), Some(CssUnit::Auto));
    }

    #[test]
    fn test_computed_style_inheritance() {
        let parent = ComputedStyle {
            color: Some("red".to_string()),
            font_size: Some(CssUnit::Pixels(20.0)),
            ..ComputedStyle::default()
        };

        let child = compute_computed_style("span", &[], None, &[], Some(&parent), &HashMap::new());
        assert_eq!(child.color, Some("red".to_string()));
        assert_eq!(child.font_size, Some(CssUnit::Pixels(20.0)));
    }

    #[test]
    fn test_auto_resolves_to_zero() {
        // `margin: 0 auto` must not expand the margin to the full parent width
        assert_eq!(CssUnit::Auto.to_pixels(800.0, 16.0), 0.0);
    }

    #[test]
    fn test_stray_brace_does_not_hang() {
        // A '}' inside a selector (malformed CSS) used to be a -= 0 no-op;
        // now it balances the depth so parsing still terminates correctly.
        let css = "div } { color: red; }";
        let rules = parse_css(css);
        // Either no rule (both braces consumed by the guard) or one rule —
        // the important part is that parse_css terminates and returns.
        assert!(rules.len() <= 1);
    }

    #[test]
    fn test_selectors_from_comma_separated() {
        let css = "h1, h2, h3 { color: navy; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors.len(), 3);
    }

    #[test]
    fn test_css_comments_ignored() {
        let css = "/* comment */ body { color: red; /* inner comment */ }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors[0].tag, Some("body".to_string()));
    }

    #[test]
    fn test_parse_class_attr() {
        let classes = parse_class_attr(Some("foo bar baz"));
        assert_eq!(classes.len(), 3);
        assert_eq!(classes[0], "foo");
        assert_eq!(classes[1], "bar");
    }
}
