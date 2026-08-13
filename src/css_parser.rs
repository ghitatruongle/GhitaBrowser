// CSS parser and style computation

use std::collections::HashMap;

/// A parsed CSS rule
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CssRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    pub specificity: (u32, u32, u32), // (id, class, tag) for cascading
}

/// One ancestor leg of a combinator selector. `direct` marks the child
/// combinator (`>`): the leg must match the immediate parent instead of any
/// ancestor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectorAncestor {
    pub tag: Option<String>,
    pub class: Option<String>,
    pub id: Option<String>,
    pub direct: bool,
}

impl SelectorAncestor {
    fn parse_compound(input: &str) -> Self {
        let compound = Selector::parse_compound(input);
        Self {
            tag: compound.tag,
            class: compound.class,
            id: compound.id,
            direct: false,
        }
    }

    fn matches(&self, tag: &str, classes: &[String], elem_id: Option<&str>) -> bool {
        if let Some(ref sel_tag) = self.tag {
            if sel_tag != "*" && sel_tag != tag {
                return false;
            }
        }
        if let Some(ref sel_class) = self.class {
            if !classes.iter().any(|c| c == sel_class) {
                return false;
            }
        }
        if let Some(ref sel_id) = self.id {
            match elem_id {
                Some(id) if id == sel_id => {}
                _ => return false,
            }
        }
        true
    }
}

/// A single CSS selector (can be compound: div.class#id, or a combinator
/// chain: `div > .item`, `nav a.highlight`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Selector {
    pub tag: Option<String>,
    pub class: Option<String>,
    pub id: Option<String>,
    pub attributes: Vec<(String, String)>, // attribute selectors [attr=value]
    /// Ancestor legs for descendant (` `) and child (`>`) combinators. The
    /// final leg is this selector; earlier legs must match ancestors.
    pub ancestors: Vec<SelectorAncestor>,
    /// `:root` pseudo-class — matches the document root element.
    pub is_root: bool,
}

impl Selector {
    /// Parse a single compound selector like "div.class#id". Chains and
    /// pseudo-classes are not handled here.
    pub fn parse_compound(input: &str) -> Self {
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
            ancestors: Vec::new(),
            is_root: false,
        }
    }

    /// Parse a full selector string: a compound selector or a combinator
    /// chain (`div > p.c`, `nav a`). Pseudo-classes beyond `:root` are not
    /// supported and make the selector match nothing.
    pub fn parse(input: &str) -> Self {
        let input = input.trim();
        if input == ":root" {
            return Selector {
                tag: None,
                class: None,
                id: None,
                attributes: Vec::new(),
                ancestors: Vec::new(),
                is_root: true,
            };
        }
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() == 1 && !input.contains('>') {
            return Self::parse_compound(input);
        }
        // Combinator chain: split on whitespace and child-combinator markers.
        let mut legs: Vec<&str> = Vec::new();
        let mut direct_flags: Vec<bool> = Vec::new();
        let mut pending_direct = false;
        for part in parts {
            if part == ">" {
                pending_direct = true;
                continue;
            }
            legs.push(part);
            direct_flags.push(pending_direct);
            pending_direct = false;
        }
        if legs.is_empty() || legs.len() > 8 {
            return Selector {
                tag: None,
                class: None,
                id: None,
                attributes: Vec::new(),
                ancestors: Vec::new(),
                is_root: false,
            };
        }
        let mut final_selector = Self::parse_compound(legs[legs.len() - 1]);
        let mut ancestors = Vec::new();
        for (index, leg) in legs[..legs.len() - 1].iter().enumerate() {
            if leg.contains(':') || leg.contains('[') {
                // Unsupported ancestor form fails closed: matches nothing.
                ancestors.clear();
                final_selector.ancestors = ancestors;
                final_selector.is_root = false;
                final_selector.tag = None;
                final_selector.class = None;
                final_selector.id = None;
                return final_selector;
            }
            let mut ancestor = SelectorAncestor::parse_compound(leg);
            ancestor.direct = direct_flags[index];
            ancestors.push(ancestor);
        }
        final_selector.ancestors = ancestors;
        final_selector
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
        self.matches_element(tag, classes, elem_id, attrs, false, &[])
    }

    /// Full match including pseudo-classes and combinator ancestors. The
    /// `is_root` flag marks the document root element (`:root`); `ancestors`
    /// supplies the element's ancestor chain (nearest first) for descendant
    /// and child combinators. Chains without a supplied ancestry never match
    /// (fail closed).
    pub fn matches_element(
        &self,
        tag: &str,
        classes: &[String],
        elem_id: Option<&str>,
        attrs: &HashMap<String, String>,
        is_root: bool,
        ancestry: &[ElementAncestry],
    ) -> bool {
        if self.is_root && !is_root {
            return false;
        }
        if !self.matches_compound(tag, classes, elem_id, attrs) {
            return false;
        }
        if self.ancestors.is_empty() {
            return true;
        }
        if ancestry.is_empty() {
            return false;
        }
        // Walk ancestor legs from the nearest ancestor outwards. Each leg
        // consumes the closest matching element; the `direct` flag forces the
        // immediate parent.
        let mut cursor = 0usize;
        for leg in &self.ancestors {
            if leg.direct {
                let Some(candidate) = ancestry.get(cursor) else {
                    return false;
                };
                if !leg.matches(&candidate.tag, &candidate.classes, candidate.id.as_deref()) {
                    return false;
                }
                cursor += 1;
            } else {
                let mut found = None;
                for (index, candidate) in ancestry.iter().enumerate().skip(cursor) {
                    if leg.matches(&candidate.tag, &candidate.classes, candidate.id.as_deref()) {
                        found = Some(index);
                        break;
                    }
                }
                let Some(index) = found else {
                    return false;
                };
                cursor = index + 1;
            }
        }
        true
    }

    fn matches_compound(
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

/// One element on the ancestor chain used to evaluate combinator selectors.
/// The chain is ordered nearest ancestor first.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ElementAncestry {
    pub tag: String,
    pub classes: Vec<String>,
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Declaration {
    pub property: String,
    pub value: String,
    #[serde(default)]
    pub important: bool,
}

/// The bounded positioning subset used by the dynamic renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PositionMode {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
}

/// Axis-aligned transform values. Rotation/skew are parsed as unsupported
/// rather than approximated, so a transformed hit box always matches pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform2D {
    pub translate_x: f64,
    pub translate_y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            translate_x: 0.0,
            translate_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

impl Transform2D {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return Some(Self::default());
        }
        let mut transform = Self::default();
        let mut remaining = value;
        let mut saw_transform = false;
        while !remaining.trim().is_empty() {
            remaining = remaining.trim_start();
            let open = remaining.find('(')?;
            let function = remaining[..open].trim().to_ascii_lowercase();
            let close = remaining[open + 1..].find(')')? + open + 1;
            let values = remaining[open + 1..close]
                .split(|character: char| character == ',' || character.is_ascii_whitespace())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>();
            match function.as_str() {
                "translate" if values.len() == 2 => {
                    transform.translate_x += parse_transform_length(values[0])?;
                    transform.translate_y += parse_transform_length(values[1])?;
                }
                "translatex" if values.len() == 1 => {
                    transform.translate_x += parse_transform_length(values[0])?;
                }
                "translatey" if values.len() == 1 => {
                    transform.translate_y += parse_transform_length(values[0])?;
                }
                "scale" if values.len() == 1 || values.len() == 2 => {
                    let scale_x = values[0].parse::<f64>().ok()?;
                    let scale_y = values
                        .get(1)
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(scale_x);
                    transform.scale_x *= scale_x.clamp(0.01, 100.0);
                    transform.scale_y *= scale_y.clamp(0.01, 100.0);
                }
                "scalex" if values.len() == 1 => {
                    transform.scale_x *= values[0].parse::<f64>().ok()?.clamp(0.01, 100.0);
                }
                "scaley" if values.len() == 1 => {
                    transform.scale_y *= values[0].parse::<f64>().ok()?.clamp(0.01, 100.0);
                }
                _ => return None,
            }
            saw_transform = true;
            remaining = &remaining[close + 1..];
        }
        saw_transform.then_some(transform)
    }
}

fn parse_transform_length(value: &str) -> Option<f64> {
    value
        .trim()
        .strip_suffix("px")
        .unwrap_or(value.trim())
        .trim()
        .parse::<f64>()
        .ok()
        .map(|value| value.clamp(-1_000_000.0, 1_000_000.0))
}

/// Computed style with many CSS properties
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub flex_direction: Option<String>,
    pub flex_wrap: Option<String>,
    pub justify_content: Option<String>,
    pub align_items: Option<String>,
    pub gap: Option<CssUnit>,
    pub grid_template_columns: Option<String>,
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
    pub float: Option<String>,

    // Dynamic layout / paint
    pub position: PositionMode,
    pub top: Option<CssUnit>,
    pub right: Option<CssUnit>,
    pub bottom: Option<CssUnit>,
    pub left: Option<CssUnit>,
    pub z_index: i32,
    pub transform: Transform2D,
    /// The raw value is parsed by the renderer into the supported property /
    /// duration subset when a style actually changes.
    pub transition: Option<String>,
    pub animation_name: Option<String>,
    pub animation_duration_ms: u64,
    pub animation_iterations: u16,

    // Advanced flex/grid subset
    pub flex_grow: f64,
    pub flex_shrink: f64,
    pub flex_basis: Option<CssUnit>,
    pub order: i32,
    pub grid_template_rows: Option<String>,
    pub grid_column_span: usize,
    pub grid_row_span: usize,

    /// CSS custom properties (`--name: value`) collected by the cascade and
    /// inherited by descendants. Bounded by the apply path.
    pub custom_properties: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
            flex_direction: None,
            flex_wrap: None,
            justify_content: None,
            align_items: None,
            gap: None,
            grid_template_columns: None,
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
            float: None,
            position: PositionMode::Static,
            top: None,
            right: None,
            bottom: None,
            left: None,
            z_index: 0,
            transform: Transform2D::default(),
            transition: None,
            animation_name: None,
            animation_duration_ms: 0,
            animation_iterations: 1,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: None,
            order: 0,
            grid_template_rows: None,
            grid_column_span: 1,
            grid_row_span: 1,
            custom_properties: HashMap::new(),
        }
    }
}

const MAX_CUSTOM_PROPERTIES: usize = 128;

impl ComputedStyle {
    pub fn apply_declaration(&mut self, decl: &Declaration) {
        self.apply_declaration_resolved(decl, &HashMap::new())
    }

    /// Apply a declaration, resolving `var(--name[, fallback])` references
    /// against the supplied custom-property map first. Custom property
    /// definitions (`--name: value`) are stored on the style and inherited
    /// by descendants through the cascade.
    pub fn apply_declaration_resolved(
        &mut self,
        decl: &Declaration,
        customs: &HashMap<String, String>,
    ) {
        let property = decl.property.trim().to_ascii_lowercase();
        if property.starts_with("--") {
            if property.len() > 128 || property.len() == 2 {
                return;
            }
            if self.custom_properties.len() < MAX_CUSTOM_PROPERTIES {
                let value = decl
                    .value
                    .chars()
                    .take(4096)
                    .collect::<String>()
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    self.custom_properties.insert(property.clone(), value);
                }
            }
            return;
        }
        let resolved = resolve_var_value(&decl.value, customs);
        let mut resolved_decl = decl.clone();
        if let Some(value) = resolved {
            resolved_decl.value = value;
        }
        self.apply_declaration_plain(&resolved_decl);
    }

    fn apply_declaration_plain(&mut self, decl: &Declaration) {
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
            "flex-direction" => self.flex_direction = Some(val.to_string()),
            "flex-wrap" => self.flex_wrap = Some(val.to_string()),
            "flex-grow" => self.flex_grow = val.parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0),
            "flex-shrink" => self.flex_shrink = val.parse::<f64>().unwrap_or(1.0).clamp(0.0, 100.0),
            "flex-basis" => self.flex_basis = CssUnit::parse(val),
            "flex" => apply_flex_shorthand(self, val),
            "order" => self.order = val.parse::<i32>().unwrap_or(0).clamp(-10_000, 10_000),
            "justify-content" => self.justify_content = Some(val.to_string()),
            "align-items" => self.align_items = Some(val.to_string()),
            "gap" | "row-gap" | "column-gap" => self.gap = CssUnit::parse(val),
            "grid-template-columns" => self.grid_template_columns = Some(val.to_string()),
            "grid-template-rows" => self.grid_template_rows = Some(val.to_string()),
            "grid-column" => self.grid_column_span = parse_grid_span(val),
            "grid-row" => self.grid_row_span = parse_grid_span(val),
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
            "float" => self.float = Some(val.to_string()),
            "opacity" => {
                self.opacity = val.parse::<f64>().ok();
            }
            "position" => {
                self.position = match val.to_ascii_lowercase().as_str() {
                    "relative" => PositionMode::Relative,
                    "absolute" => PositionMode::Absolute,
                    "fixed" => PositionMode::Fixed,
                    _ => PositionMode::Static,
                }
            }
            "top" => self.top = CssUnit::parse(val),
            "right" => self.right = CssUnit::parse(val),
            "bottom" => self.bottom = CssUnit::parse(val),
            "left" => self.left = CssUnit::parse(val),
            "z-index" => self.z_index = val.parse::<i32>().unwrap_or(0).clamp(-10_000, 10_000),
            "transform" => {
                if let Some(transform) = Transform2D::parse(val) {
                    self.transform = transform;
                }
            }
            "transition" | "transition-property" | "transition-duration" => {
                self.transition = Some(val.to_string())
            }
            "animation" => apply_animation_shorthand(self, val),
            "animation-name" => self.animation_name = Some(val.to_string()),
            "animation-duration" => self.animation_duration_ms = parse_duration_ms(val),
            "animation-iteration-count" => {
                self.animation_iterations = parse_animation_iterations(val)
            }

            _ => {} // Unknown properties are silently ignored
        }
    }
}

/// Resolve every `var(--name)` / `var(--name, fallback)` reference in a
/// declaration value against the custom-property map. Returns the resolved
/// value, or `None` when the value contains no references. Unresolvable
/// references without a fallback yield an empty string (the declaration is
/// dropped by the property parser, matching `invalid at computed-value time`).
fn resolve_var_value(value: &str, customs: &HashMap<String, String>) -> Option<String> {
    if !value.contains("var(") {
        return None;
    }
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    let mut resolved_any = false;
    for _ in 0..16 {
        let Some(start) = remaining.find("var(") else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 4..];
        let Some(close) = find_var_close(after) else {
            output.push_str(remaining);
            return Some(output);
        };
        let body = &after[..close];
        let (name, fallback) = match body.split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (body.trim(), None),
        };
        if !name.starts_with("--") {
            // Invalid custom property name: the whole declaration is invalid.
            return Some(String::new());
        }
        if let Some(value) = customs.get(name) {
            output.push_str(value);
            resolved_any = true;
        } else if let Some(fallback) = fallback {
            if fallback.contains("var(") {
                // Fallback may itself reference variables; resolve it.
                let nested =
                    resolve_var_value(fallback, customs).unwrap_or_else(|| fallback.to_string());
                output.push_str(&nested);
                resolved_any = true;
            } else {
                output.push_str(fallback);
                resolved_any = true;
            }
        } else {
            // Unresolvable without fallback: declaration is invalid.
            return Some(String::new());
        }
        remaining = &after[close + 1..];
    }
    if remaining.contains("var(") {
        return Some(String::new());
    }
    if resolved_any {
        Some(output)
    } else {
        None
    }
}

/// Find the `)` that closes a `var(` body, ignoring nested `var(` calls.
fn find_var_close(after: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in after.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn apply_flex_shorthand(style: &mut ComputedStyle, value: &str) {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 1 {
        if let Ok(grow) = parts[0].parse::<f64>() {
            style.flex_grow = grow.clamp(0.0, 100.0);
            return;
        }
        style.flex_basis = CssUnit::parse(parts[0]);
        return;
    }
    if let Some(grow) = parts.first().and_then(|value| value.parse::<f64>().ok()) {
        style.flex_grow = grow.clamp(0.0, 100.0);
    }
    if let Some(shrink) = parts.get(1).and_then(|value| value.parse::<f64>().ok()) {
        style.flex_shrink = shrink.clamp(0.0, 100.0);
    }
    if let Some(basis) = parts.get(2) {
        style.flex_basis = CssUnit::parse(basis);
    }
}

fn parse_grid_span(value: &str) -> usize {
    value
        .to_ascii_lowercase()
        .strip_prefix("span")
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 12)
}

fn parse_duration_ms(value: &str) -> u64 {
    let value = value.trim().split(',').next().unwrap_or_default().trim();
    if let Some(milliseconds) = value.strip_suffix("ms") {
        return milliseconds
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.clamp(0.0, 60_000.0) as u64)
            .unwrap_or(0);
    }
    if let Some(seconds) = value.strip_suffix('s') {
        return seconds
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| (value * 1_000.0).clamp(0.0, 60_000.0) as u64)
            .unwrap_or(0);
    }
    0
}

fn parse_animation_iterations(value: &str) -> u16 {
    if value.trim().eq_ignore_ascii_case("infinite") {
        return 1_000;
    }
    value.trim().parse::<u16>().unwrap_or(1).clamp(1, 1_000)
}

fn apply_animation_shorthand(style: &mut ComputedStyle, value: &str) {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    let mut name = None;
    for part in &parts {
        let duration = parse_duration_ms(part);
        if duration > 0 {
            style.animation_duration_ms = duration;
        } else if *part == "infinite" || part.parse::<u16>().is_ok() {
            style.animation_iterations = parse_animation_iterations(part);
        } else if !matches!(
            *part,
            "linear"
                | "ease"
                | "ease-in"
                | "ease-out"
                | "ease-in-out"
                | "forwards"
                | "backwards"
                | "both"
                | "normal"
                | "alternate"
        ) {
            name = Some((*part).to_string());
        }
    }
    style.animation_name = name;
}

/// Remove `/* ... */` comments from CSS text. Comments do not nest in CSS.
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let chars: Vec<char> = css.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Parse CSS string into rules
pub fn parse_css(css: &str) -> Vec<CssRule> {
    parse_css_with_media(css, 0)
}

/// Parse CSS, evaluating the bounded `@media` subset (`screen`, `all`, `not`,
/// `and`, `(min-width: Npx)`, `(max-width: Npx)`, comma-separated OR lists)
/// against the given viewport width. A viewport of `0` means "no viewport":
/// media rules are skipped entirely (fail closed).
pub fn parse_css_with_media(css: &str, viewport_width: u32) -> Vec<CssRule> {
    let mut rules = Vec::new();
    // Strip `/* ... */` comments up front. The old code only skipped them at
    // the top of the outer loop, so a comment anywhere else (inside a
    // selector like `div /* x */ .c { ... }` or in a declaration value)
    // poisoned the token stream and killed the rule.
    let stripped = strip_css_comments(css);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return rules;
    }

    let mut pos = 0;
    let chars: Vec<char> = stripped.chars().collect();
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

        // At-rules: @media is evaluated against the viewport; the other
        // at-rules (@supports, @keyframes, @import, ...) are skipped whole.
        let head = selector_str.trim_start();
        if head.starts_with('@') {
            if head.starts_with("@media") && viewport_width > 0 {
                let condition = head["@media".len()..].trim().trim_end_matches('{').trim();
                if media_query_matches(condition, viewport_width) {
                    // Recursively parse the media block body as ordinary rules.
                    let body_start = pos;
                    let mut depth = 1;
                    while pos < len && depth > 0 {
                        if chars[pos] == '{' {
                            depth += 1;
                        } else if chars[pos] == '}' {
                            depth -= 1;
                        }
                        pos += 1;
                    }
                    let body: String = chars[body_start..pos.saturating_sub(1)].iter().collect();
                    rules.extend(parse_css_with_media(&body, viewport_width));
                    continue;
                }
            }
            let mut depth = 1;
            while pos < len && depth > 0 {
                if chars[pos] == '{' {
                    depth += 1;
                } else if chars[pos] == '}' {
                    depth -= 1;
                }
                pos += 1;
            }
            continue;
        }

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
            let value = value.trim();
            let (value, important) = strip_important(value);
            let value = value.to_string();
            if pos < len && chars[pos] == ';' {
                pos += 1;
            }

            if !property.is_empty() && !value.is_empty() {
                declarations.push(Declaration {
                    property,
                    value,
                    important,
                });
            }
        }
        if pos < len && chars[pos] == '}' {
            pos += 1;
        }

        // Parse selector string into individual selectors (comma-separated).
        // Selectors with no effective parts (`p,` trailing comma, empty
        // groups) are dropped — otherwise they'd match EVERY element and
        // recolor the whole page.
        let selector_str = selector_str.trim();
        if !selector_str.is_empty() {
            let selectors: Vec<Selector> = selector_str
                .split(',')
                .map(|s| Selector::parse(s.trim()))
                .filter(|sel| {
                    sel.tag.is_some()
                        || sel.class.is_some()
                        || sel.id.is_some()
                        || !sel.attributes.is_empty()
                        || sel.is_root
                })
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
    compute_computed_style_with_ancestors(
        element_tag,
        element_classes,
        element_id,
        rules,
        parent_style,
        element_attrs,
        false,
        &[],
    )
}

/// Extended cascade with `:root` and combinator-ancestor context for Phase 21
/// selectors. `is_root` marks the document root element; `ancestry` supplies
/// the ancestor chain (nearest first) for descendant/child combinators.
#[allow(clippy::too_many_arguments)]
pub fn compute_computed_style_with_ancestors(
    element_tag: &str,
    element_classes: &[String],
    element_id: Option<&str>,
    rules: &[CssRule],
    parent_style: Option<&ComputedStyle>,
    element_attrs: &HashMap<String, String>,
    is_root: bool,
    ancestry: &[ElementAncestry],
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
            custom_properties: parent.custom_properties.clone(),
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
    if matches!(element_tag, "input" | "button" | "select" | "textarea") {
        style.display = Some("inline-block".to_string());
        style.background_color = Some("#ffffff".to_string());
        style.border_width = Some(CssUnit::Pixels(1.0));
        style.border_style = Some("solid".to_string());
        style.border_color = Some("#9aa0a6".to_string());
        style.padding_top = Some(CssUnit::Pixels(6.0));
        style.padding_right = Some(CssUnit::Pixels(8.0));
        style.padding_bottom = Some(CssUnit::Pixels(6.0));
        style.padding_left = Some(CssUnit::Pixels(8.0));
    }

    // Apply matching rules, keyed by the specificity of the SPECIFIC
    // selector that matched. A grouped rule like `#x, p { color: red }`
    // must not give its plain `p` leg the `#x` specificity — otherwise it
    // would wrongly outrank `p.mine { color: blue }` on <p class="mine">.
    let mut matching_rules: Vec<((u32, u32, u32), &CssRule)> = Vec::new();
    for rule in rules {
        for selector in &rule.selectors {
            if selector.matches_element(
                element_tag,
                element_classes,
                element_id,
                element_attrs,
                is_root,
                ancestry,
            ) {
                matching_rules.push((selector.specificity(), rule));
                break;
            }
        }
    }

    // Sort by the matched selector's specificity (stably, so earlier rules
    // win ties — the "cascade" order).
    matching_rules.sort_by_key(|(spec, _)| *spec);

    // Pass 1: collect custom property definitions from the matching rules so
    // `var()` references anywhere in the cascade resolve against the final
    // values (custom properties participate in the cascade like other props).
    for (_spec, rule) in &matching_rules {
        for decl in &rule.declarations {
            if decl.property.trim().starts_with("--") {
                style.apply_declaration_resolved(decl, &style.custom_properties.clone());
            }
        }
    }

    // Pass 2: apply regular declarations with `var()` resolution. Important
    // declarations are applied after every normal declaration so a later
    // non-important rule cannot override them.
    // Inline `style="..."` attribute — the highest-precedence source
    // (stronger than any stylesheet rule). It was previously ignored, so
    // pages styling elements with style="" rendered unstyled.
    let inline_declarations = element_attrs
        .get("style")
        .map(|inline| parse_inline_declarations(inline))
        .unwrap_or_default();
    for decl in &inline_declarations {
        if decl.property.trim().starts_with("--") {
            style.apply_declaration_resolved(decl, &style.custom_properties.clone());
        }
    }

    // Apply author declarations in bounded CSS cascade order: normal rules,
    // normal inline style, important rules, important inline style.
    for (_spec, rule) in &matching_rules {
        for decl in &rule.declarations {
            if !decl.property.trim().starts_with("--") && !decl.important {
                style.apply_declaration_resolved(decl, &style.custom_properties.clone());
            }
        }
    }
    for decl in &inline_declarations {
        if !decl.property.trim().starts_with("--") && !decl.important {
            style.apply_declaration_resolved(decl, &style.custom_properties.clone());
        }
    }
    for (_spec, rule) in &matching_rules {
        for decl in &rule.declarations {
            if !decl.property.trim().starts_with("--") && decl.important {
                style.apply_declaration_resolved(decl, &style.custom_properties.clone());
            }
        }
    }
    for decl in &inline_declarations {
        if !decl.property.trim().starts_with("--") && decl.important {
            style.apply_declaration_resolved(decl, &style.custom_properties.clone());
        }
    }

    style
}

/// Parse a `style=""` attribute ("prop: value; prop2: value2") into
/// declarations. Malformed fragments are skipped.
fn parse_inline_declarations(s: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((prop, value)) = part.split_once(':') {
            let prop = prop.trim().to_lowercase();
            let (value, important) = strip_important(value.trim());
            let value = value.to_string();
            if !prop.is_empty() && !value.is_empty() {
                out.push(Declaration {
                    property: prop,
                    value,
                    important,
                });
            }
        }
    }
    out
}

fn strip_important(value: &str) -> (&str, bool) {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(prefix) = lower.strip_suffix("!important") {
        let split = prefix.len();
        (trimmed[..split].trim_end(), true)
    } else {
        (trimmed, false)
    }
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

/// Evaluate the bounded media-query subset against a viewport width.
/// Supported: `screen`, `all`, `not <expr>`, `<expr> and <expr>`,
/// `(min-width: Npx)`, `(max-width: Npx)` and comma-separated OR lists.
/// Anything else evaluates to false (fail closed).
fn media_query_matches(condition: &str, viewport_width: u32) -> bool {
    let alternatives = condition.split(',');
    let mut any = false;
    for alternative in alternatives {
        let mut matches = true;
        for part in alternative.split("and") {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let part_matches = if let Some(rest) = part.strip_prefix("not ") {
                !media_term_matches(rest.trim(), viewport_width)
            } else {
                media_term_matches(part, viewport_width)
            };
            matches &= part_matches;
        }
        any |= matches;
    }
    any
}

fn media_term_matches(term: &str, viewport_width: u32) -> bool {
    let term = term.trim();
    if term.eq_ignore_ascii_case("all") || term.eq_ignore_ascii_case("screen") {
        return true;
    }
    if term.starts_with('(') && term.ends_with(')') {
        let inner = term[1..term.len() - 1].trim();
        if let Some(rest) = inner.strip_prefix("max-width:") {
            return parse_media_length(rest).is_some_and(|max| viewport_width <= max);
        }
        if let Some(rest) = inner.strip_prefix("min-width:") {
            return parse_media_length(rest).is_some_and(|min| viewport_width >= min);
        }
    }
    false
}

fn parse_media_length(value: &str) -> Option<u32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    number
        .parse::<f64>()
        .ok()
        .map(|value| value.max(0.0) as u32)
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

    #[test]
    fn test_empty_selector_in_group_is_dropped() {
        // `p, { color: red }` — the empty fragment after the comma must NOT
        // become a match-everything rule.
        let rules = parse_css("p, { color: red; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors.len(), 1);
        assert_eq!(rules[0].selectors[0].tag.as_deref(), Some("p"));
    }

    #[test]
    fn test_comment_inside_selector_does_not_kill_rule() {
        // A comment between selector parts used to poison the selector string.
        // `div .c` is a descendant combinator chain in the Phase 21 profile.
        let css = "div /* x */ .c { color: red; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        let sel = &rules[0].selectors[0];
        assert_eq!(sel.class.as_deref(), Some("c"));
        assert_eq!(sel.ancestors.len(), 1);
        assert_eq!(sel.ancestors[0].tag.as_deref(), Some("div"));
    }

    #[test]
    fn test_comment_inside_compound_selector_keeps_compound() {
        // Without whitespace the comment stays inside one compound selector.
        let css = "div/* x */.c { color: red; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1);
        let sel = &rules[0].selectors[0];
        assert_eq!(sel.tag.as_deref(), Some("div"));
        assert_eq!(sel.class.as_deref(), Some("c"));
        assert!(sel.ancestors.is_empty());
    }

    #[test]
    fn test_at_rule_is_does_not_mangle_following_rules() {
        // @media used to swallow a rule and poison what followed.
        let css = "@media screen { body { color: red; } } p { color: blue; }";
        let rules = parse_css(css);
        // Only the plain `p { ... }` rule survives (media rules are skipped)
        assert_eq!(rules.len(), 1, "rules: {:?}", rules);
        assert_eq!(rules[0].selectors[0].tag.as_deref(), Some("p"));
    }

    #[test]
    fn test_grouped_selector_specificity_is_per_selector() {
        // `#x, p { color: red }` — on a plain <p class="mine"> the `p` leg
        // must NOT inherit the `#x` specificity; `p.mine` (class) must win.
        let css = "#x, p { color: red; } p.mine { color: blue; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 2);

        let style = compute_computed_style(
            "p",
            &["mine".to_string()],
            None,
            &rules,
            None,
            &HashMap::new(),
        );
        assert_eq!(
            style.color.as_deref(),
            Some("blue"),
            "p.mine (specificity 0,1,1) must beat the grouped rule's p leg (0,0,1)"
        );

        // The group's #x leg still wins when the ID matches.
        let with_id = compute_computed_style("p", &[], Some("x"), &rules, None, &HashMap::new());
        assert_eq!(with_id.color.as_deref(), Some("red"));
    }

    #[test]
    fn dynamic_rendering_properties_are_bounded_and_parsed() {
        let rules = parse_css(
            ".card { position:absolute; left:12px; top:8px; transform:translateX(4px) scale(2); \
             transition: opacity 150ms; animation: ghita-fade-in 200ms 2; \
             flex:2 1 30px; order:-1; grid-column:span 2; grid-row:span 3; }",
        );
        let style = compute_computed_style(
            "div",
            &["card".to_string()],
            None,
            &rules,
            None,
            &HashMap::new(),
        );
        assert_eq!(style.position, PositionMode::Absolute);
        assert_eq!(style.left, Some(CssUnit::Pixels(12.0)));
        assert_eq!(style.top, Some(CssUnit::Pixels(8.0)));
        assert_eq!(style.transform.translate_x, 4.0);
        assert_eq!(style.transform.scale_x, 2.0);
        assert_eq!(style.transition.as_deref(), Some("opacity 150ms"));
        assert_eq!(style.animation_name.as_deref(), Some("ghita-fade-in"));
        assert_eq!(style.animation_duration_ms, 200);
        assert_eq!(style.animation_iterations, 2);
        assert_eq!(style.flex_grow, 2.0);
        assert_eq!(style.order, -1);
        assert_eq!(style.grid_column_span, 2);
        assert_eq!(style.grid_row_span, 3);
    }
}
