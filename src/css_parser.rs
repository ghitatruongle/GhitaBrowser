// CSS parser, selector matching, cascade, inheritance, and value computation.
// Standards-oriented implementation supporting full compound selectors, combinators,
// at-rules, cascade origins & layers, CSS-wide keywords, shorthands, custom properties,
// typed units, math expressions (calc/min/max/clamp), color grammar, and diagnostics.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// A parsed CSS rule with origin, cascade layer, source order, selectors, declarations, and specificity.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CssRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    pub specificity: (u32, u32, u32), // (id, class, tag)
    #[serde(default)]
    pub origin: CssOrigin,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub layer_order: Option<usize>,
    #[serde(default)]
    pub source_order: usize,
}

/// Cascade origin according to CSS Cascading and Inheritance Level 5.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum CssOrigin {
    #[default]
    Author,
    User,
    UserAgent,
}

/// Attribute matching operators in selectors.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttributeMatch {
    Presence,  // [attr]
    Exact,     // [attr=val]
    Includes,  // [attr~=val] (whitespace-separated words)
    DashMatch, // [attr|=val] (exact or prefix followed by '-')
    Prefix,    // [attr^=val]
    Suffix,    // [attr$=val]
    Substring, // [attr*=val]
}

/// Case sensitivity modifier for attribute selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CaseSensitivity {
    #[default]
    Default,
    CaseSensitive,   // 's'
    CaseInsensitive, // 'i'
}

/// A parsed attribute selector component.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttributeSelector {
    pub name: String,
    pub operator: AttributeMatch,
    pub value: String,
    pub case_sensitivity: CaseSensitivity,
}

/// Combinator linking selector components (evaluated right-to-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Combinator {
    Descendant,      // ' '
    Child,           // '>'
    AdjacentSibling, // '+'
    GeneralSibling,  // '~'
}

/// Pseudo-classes supported by the engine.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PseudoClass {
    Root,
    Empty,
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthChild {
        a: i32,
        b: i32,
        of_selector: Option<Box<Selector>>,
    },
    NthLastChild {
        a: i32,
        b: i32,
        of_selector: Option<Box<Selector>>,
    },
    NthOfType {
        a: i32,
        b: i32,
    },
    NthLastOfType {
        a: i32,
        b: i32,
    },
    Hover,
    Active,
    Focus,
    FocusVisible,
    FocusWithin,
    Enabled,
    Disabled,
    Checked,
    Indeterminate,
    Link,
    Visited,
    Target,
    Valid,
    Invalid,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    Where(Vec<Selector>),
    Has(Vec<Selector>),
    Lang(String),
    Custom(String),
}

/// Pseudo-elements supported by the engine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PseudoElement {
    Before,
    After,
    Placeholder,
    FirstLetter,
    FirstLine,
    Marker,
    Selection,
    Custom(String),
}

/// A single compound selector component (e.g. `div.highlight#main[data-active]:hover::before`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CompoundSelector {
    pub tag: Option<String>,
    pub namespace: Option<String>,
    pub classes: Vec<String>,
    pub id: Option<String>,
    pub attributes: Vec<AttributeSelector>,
    pub pseudo_classes: Vec<PseudoClass>,
    pub pseudo_element: Option<PseudoElement>,
    pub is_root: bool,
}

impl CompoundSelector {
    pub fn is_empty(&self) -> bool {
        self.tag.is_none()
            && self.classes.is_empty()
            && self.id.is_none()
            && self.attributes.is_empty()
            && self.pseudo_classes.is_empty()
            && self.pseudo_element.is_none()
            && !self.is_root
    }

    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut ids = if self.id.is_some() { 1 } else { 0 };
        let mut classes = self.classes.len() as u32 + self.attributes.len() as u32;
        let mut tags = if self.tag.is_some() && self.tag.as_deref() != Some("*") {
            1
        } else {
            0
        };

        if self.is_root {
            classes += 1;
        }
        if self.pseudo_element.is_some() {
            tags += 1;
        }

        for pc in &self.pseudo_classes {
            match pc {
                PseudoClass::Root
                | PseudoClass::Empty
                | PseudoClass::FirstChild
                | PseudoClass::LastChild
                | PseudoClass::OnlyChild
                | PseudoClass::FirstOfType
                | PseudoClass::LastOfType
                | PseudoClass::OnlyOfType
                | PseudoClass::NthChild { .. }
                | PseudoClass::NthLastChild { .. }
                | PseudoClass::NthOfType { .. }
                | PseudoClass::NthLastOfType { .. }
                | PseudoClass::Hover
                | PseudoClass::Active
                | PseudoClass::Focus
                | PseudoClass::FocusVisible
                | PseudoClass::FocusWithin
                | PseudoClass::Enabled
                | PseudoClass::Disabled
                | PseudoClass::Checked
                | PseudoClass::Indeterminate
                | PseudoClass::Link
                | PseudoClass::Visited
                | PseudoClass::Target
                | PseudoClass::Valid
                | PseudoClass::Invalid
                | PseudoClass::Required
                | PseudoClass::Optional
                | PseudoClass::ReadOnly
                | PseudoClass::ReadWrite
                | PseudoClass::Lang(_)
                | PseudoClass::Custom(_) => {
                    classes += 1;
                }
                PseudoClass::Not(list) | PseudoClass::Is(list) | PseudoClass::Has(list) => {
                    let mut max_spec = (0, 0, 0);
                    for s in list {
                        let spec = s.specificity();
                        if spec > max_spec {
                            max_spec = spec;
                        }
                    }
                    ids += max_spec.0;
                    classes += max_spec.1;
                    tags += max_spec.2;
                }
                PseudoClass::Where(_) => {
                    // :where() has zero specificity
                }
            }
        }

        (ids, classes, tags)
    }

    pub fn matches_element_context(&self, ctx: &ElementMatchingContext) -> bool {
        if self.is_root && !ctx.is_root {
            return false;
        }

        // Tag check (HTML tags are matched case-insensitively)
        if let Some(ref sel_tag) = self.tag {
            if sel_tag != "*" && !sel_tag.eq_ignore_ascii_case(ctx.tag) {
                return false;
            }
        }

        // Class check (case-sensitive in HTML/CSS)
        for sel_class in &self.classes {
            if !ctx.classes.iter().any(|c| c == sel_class) {
                return false;
            }
        }

        // ID check (case-sensitive)
        if let Some(ref sel_id) = self.id {
            match ctx.id {
                Some(id) if id == sel_id => {}
                _ => return false,
            }
        }

        // Attribute checks
        for attr in &self.attributes {
            let actual = match ctx.attrs.get(&attr.name.to_ascii_lowercase()) {
                Some(v) => v.as_str(),
                None => match ctx.attrs.get(&attr.name) {
                    Some(v) => v.as_str(),
                    None => return false,
                },
            };

            let matches = match attr.operator {
                AttributeMatch::Presence => true,
                AttributeMatch::Exact => match attr.case_sensitivity {
                    CaseSensitivity::CaseInsensitive => actual.eq_ignore_ascii_case(&attr.value),
                    _ => actual == attr.value,
                },
                AttributeMatch::Includes => match attr.case_sensitivity {
                    CaseSensitivity::CaseInsensitive => actual
                        .split_ascii_whitespace()
                        .any(|w| w.eq_ignore_ascii_case(&attr.value)),
                    _ => actual.split_ascii_whitespace().any(|w| w == attr.value),
                },
                AttributeMatch::DashMatch => match attr.case_sensitivity {
                    CaseSensitivity::CaseInsensitive => {
                        // Fold to lowercase and compare on bytes; direct
                        // slicing by another string's byte length can split
                        // a multi-byte character and panic.
                        let actual_folded = actual.to_ascii_lowercase();
                        let value_folded = attr.value.to_ascii_lowercase();
                        actual_folded == value_folded
                            || actual_folded
                                .strip_prefix(value_folded.as_str())
                                .is_some_and(|rest| rest.starts_with('-'))
                    }
                    _ => {
                        actual == attr.value
                            || (actual.starts_with(&attr.value)
                                && actual.as_bytes().get(attr.value.len()) == Some(&b'-'))
                    }
                },
                AttributeMatch::Prefix => {
                    if attr.value.is_empty() {
                        false
                    } else {
                        match attr.case_sensitivity {
                            CaseSensitivity::CaseInsensitive => actual
                                .get(..attr.value.len())
                                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&attr.value)),
                            _ => actual.starts_with(&attr.value),
                        }
                    }
                }
                AttributeMatch::Suffix => {
                    if attr.value.is_empty() {
                        false
                    } else {
                        match attr.case_sensitivity {
                            CaseSensitivity::CaseInsensitive => {
                                actual.len() >= attr.value.len()
                                    && actual
                                        .get(actual.len() - attr.value.len()..)
                                        .is_some_and(|suffix| {
                                            suffix.eq_ignore_ascii_case(&attr.value)
                                        })
                            }
                            _ => actual.ends_with(&attr.value),
                        }
                    }
                }
                AttributeMatch::Substring => {
                    if attr.value.is_empty() {
                        false
                    } else {
                        match attr.case_sensitivity {
                            CaseSensitivity::CaseInsensitive => actual
                                .to_ascii_lowercase()
                                .contains(&attr.value.to_ascii_lowercase()),
                            _ => actual.contains(&attr.value),
                        }
                    }
                }
            };

            if !matches {
                return false;
            }
        }

        // Pseudo-class checks
        for pc in &self.pseudo_classes {
            if !match_pseudo_class(pc, ctx) {
                return false;
            }
        }

        true
    }
}

fn match_pseudo_class(pc: &PseudoClass, ctx: &ElementMatchingContext) -> bool {
    match pc {
        PseudoClass::Root => ctx.is_root,
        PseudoClass::Empty => ctx.is_empty,
        PseudoClass::FirstChild => ctx.index_in_parent == 1,
        PseudoClass::LastChild => ctx.siblings_after == 0 && ctx.total_siblings > 0,
        PseudoClass::OnlyChild => ctx.total_siblings == 1,
        PseudoClass::FirstOfType => ctx.type_index_in_parent == 1,
        PseudoClass::LastOfType => {
            ctx.total_type_siblings > 0 && ctx.type_index_in_parent == ctx.total_type_siblings
        }
        PseudoClass::OnlyOfType => ctx.total_type_siblings == 1,
        PseudoClass::NthChild { a, b, .. } => eval_nth(*a, *b, ctx.index_in_parent),
        PseudoClass::NthLastChild { a, b, .. } => {
            let index_from_end = ctx.siblings_after + 1;
            eval_nth(*a, *b, index_from_end)
        }
        PseudoClass::NthOfType { a, b } => eval_nth(*a, *b, ctx.type_index_in_parent),
        PseudoClass::NthLastOfType { a, b } => {
            let index_from_end = ctx
                .total_type_siblings
                .saturating_sub(ctx.type_index_in_parent)
                + 1;
            eval_nth(*a, *b, index_from_end)
        }
        PseudoClass::Hover => ctx.is_hovered,
        PseudoClass::Active => ctx.is_active,
        PseudoClass::Focus => ctx.is_focused,
        PseudoClass::FocusVisible => ctx.is_focused,
        PseudoClass::FocusWithin => ctx.is_focused,
        PseudoClass::Enabled => !ctx.is_disabled,
        PseudoClass::Disabled => ctx.is_disabled,
        PseudoClass::Checked => ctx.is_checked,
        PseudoClass::Indeterminate => false,
        PseudoClass::Link => {
            ctx.tag.eq_ignore_ascii_case("a") && ctx.attrs.contains_key("href") && !ctx.is_visited
        }
        PseudoClass::Visited => {
            ctx.tag.eq_ignore_ascii_case("a") && ctx.attrs.contains_key("href") && ctx.is_visited
        }
        PseudoClass::Target => ctx.is_target,
        PseudoClass::Valid => !ctx.is_disabled,
        PseudoClass::Invalid => false,
        PseudoClass::Required => ctx.attrs.contains_key("required"),
        PseudoClass::Optional => !ctx.attrs.contains_key("required"),
        PseudoClass::ReadOnly => ctx.attrs.contains_key("readonly") || ctx.is_disabled,
        PseudoClass::ReadWrite => !ctx.attrs.contains_key("readonly") && !ctx.is_disabled,
        PseudoClass::Not(list) => !list.iter().any(|sel| sel.matches_context(ctx)),
        PseudoClass::Is(list) | PseudoClass::Where(list) => {
            list.iter().any(|sel| sel.matches_context(ctx))
        }
        PseudoClass::Has(_) => true,
        PseudoClass::Lang(target_lang) => {
            if let Some(lang) = ctx.attrs.get("lang") {
                let lang_folded = lang.to_ascii_lowercase();
                let target_folded = target_lang.to_ascii_lowercase();
                // Compare without slicing so multi-byte attributes cannot
                // produce a non-char-boundary panic.
                lang_folded == target_folded
                    || lang_folded
                        .strip_prefix(target_folded.as_str())
                        .is_some_and(|rest| rest.starts_with('-'))
            } else {
                false
            }
        }
        PseudoClass::Custom(_) => true,
    }
}

fn eval_nth(a: i32, b: i32, index: usize) -> bool {
    let index = index as i32;
    if a == 0 {
        index == b
    } else {
        let diff = index - b;
        if a > 0 {
            diff >= 0 && diff % a == 0
        } else {
            diff <= 0 && diff % a == 0
        }
    }
}

/// Backwards-compatible ancestor representation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectorAncestor {
    pub tag: Option<String>,
    pub class: Option<String>,
    pub id: Option<String>,
    pub direct: bool,
}

impl SelectorAncestor {
    fn matches(&self, tag: &str, classes: &[String], elem_id: Option<&str>) -> bool {
        if let Some(ref sel_tag) = self.tag {
            if sel_tag != "*" && !sel_tag.eq_ignore_ascii_case(tag) {
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

/// A parsed CSS selector (compound selector or combinator chain).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Selector {
    // Backwards-compatible public fields
    pub tag: Option<String>,
    pub class: Option<String>,
    pub id: Option<String>,
    pub attributes: Vec<(String, String)>,
    pub ancestors: Vec<SelectorAncestor>,
    pub is_root: bool,

    // Advanced selector components and combinators (right-to-left chain)
    #[serde(default)]
    pub components: Vec<CompoundSelector>,
    #[serde(default)]
    pub combinators: Vec<Combinator>,
}

/// Element matching context providing full hierarchy, sibling, and state data.
pub struct ElementMatchingContext<'a> {
    pub tag: &'a str,
    pub classes: &'a [String],
    pub id: Option<&'a str>,
    pub attrs: &'a HashMap<String, String>,
    pub is_root: bool,
    pub ancestors: &'a [ElementAncestry],
    pub index_in_parent: usize,
    pub siblings_after: usize,
    pub total_siblings: usize,
    pub type_index_in_parent: usize,
    pub total_type_siblings: usize,
    pub is_hovered: bool,
    pub is_focused: bool,
    pub is_active: bool,
    pub is_checked: bool,
    pub is_disabled: bool,
    pub is_empty: bool,
    pub is_target: bool,
    pub is_visited: bool,
    /// Prior siblings, nearest first, for `+` / `~` combinators.
    pub previous_siblings: &'a [ElementAncestry],
}

impl<'a> ElementMatchingContext<'a> {
    pub fn simple(
        tag: &'a str,
        classes: &'a [String],
        id: Option<&'a str>,
        attrs: &'a HashMap<String, String>,
        is_root: bool,
        ancestors: &'a [ElementAncestry],
    ) -> Self {
        let is_disabled = attrs.contains_key("disabled");
        let is_checked = attrs.contains_key("checked");
        Self {
            tag,
            classes,
            id,
            attrs,
            is_root,
            ancestors,
            index_in_parent: 1,
            siblings_after: 0,
            total_siblings: 1,
            type_index_in_parent: 1,
            total_type_siblings: 1,
            is_hovered: false,
            is_focused: false,
            is_active: false,
            is_checked,
            is_disabled,
            is_empty: false,
            is_target: false,
            is_visited: false,
            // Empty slice literals are 'static, so no sibling data needed.
            previous_siblings: &[],
        }
    }

    /// Full constructor carrying real sibling-position facts.
    pub fn with_siblings(
        tag: &'a str,
        classes: &'a [String],
        id: Option<&'a str>,
        attrs: &'a HashMap<String, String>,
        is_root: bool,
        ancestors: &'a [ElementAncestry],
        siblings: &'a SiblingContext,
    ) -> Self {
        let is_disabled = attrs.contains_key("disabled");
        let is_checked = attrs.contains_key("checked");
        Self {
            tag,
            classes,
            id,
            attrs,
            is_root,
            ancestors,
            index_in_parent: siblings.index_in_parent.max(1),
            siblings_after: siblings.siblings_after,
            total_siblings: siblings.total_siblings,
            type_index_in_parent: siblings.type_index_in_parent.max(1),
            total_type_siblings: siblings.total_type_siblings,
            is_hovered: false,
            is_focused: false,
            is_active: false,
            is_checked,
            is_disabled,
            is_empty: false,
            is_target: false,
            is_visited: false,
            previous_siblings: siblings.previous_siblings,
        }
    }
}

impl Selector {
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed == ":root" {
            let comp = CompoundSelector {
                is_root: true,
                ..CompoundSelector::default()
            };
            return Selector {
                tag: None,
                class: None,
                id: None,
                attributes: Vec::new(),
                ancestors: Vec::new(),
                is_root: true,
                components: vec![comp],
                combinators: Vec::new(),
            };
        }

        if let Some(parsed) = parse_selector_chain(trimmed) {
            if !parsed.components.is_empty() {
                return parsed;
            }
        }

        Self::parse_compound(trimmed)
    }

    pub fn parse_compound(input: &str) -> Self {
        let (comp, attrs_legacy) = parse_single_compound(input);
        let first_class = comp.classes.first().cloned();
        let is_root = comp.is_root;
        Selector {
            tag: comp.tag.clone(),
            class: first_class,
            id: comp.id.clone(),
            attributes: attrs_legacy,
            ancestors: Vec::new(),
            is_root,
            components: vec![comp],
            combinators: Vec::new(),
        }
    }

    pub fn specificity(&self) -> (u32, u32, u32) {
        if self.components.is_empty() {
            let id = if self.id.is_some() { 1 } else { 0 };
            let class = if self.class.is_some() { 1 } else { 0 } + self.attributes.len() as u32;
            let tag = if self.tag.is_some() && self.tag.as_deref() != Some("*") {
                1
            } else {
                0
            };
            return (id, class, tag);
        }

        let mut total = (0, 0, 0);
        for comp in &self.components {
            let s = comp.specificity();
            total.0 += s.0;
            total.1 += s.1;
            total.2 += s.2;
        }
        total
    }

    pub fn matches(
        &self,
        tag: &str,
        classes: &[String],
        elem_id: Option<&str>,
        attrs: &HashMap<String, String>,
    ) -> bool {
        self.matches_element(tag, classes, elem_id, attrs, false, &[])
    }

    pub fn matches_element(
        &self,
        tag: &str,
        classes: &[String],
        elem_id: Option<&str>,
        attrs: &HashMap<String, String>,
        is_root: bool,
        ancestry: &[ElementAncestry],
    ) -> bool {
        let ctx = ElementMatchingContext::simple(tag, classes, elem_id, attrs, is_root, ancestry);
        self.matches_context(&ctx)
    }

    pub fn matches_context(&self, ctx: &ElementMatchingContext) -> bool {
        if self.is_root && !ctx.is_root {
            return false;
        }

        if self.components.is_empty() {
            return self.matches_legacy(
                ctx.tag,
                ctx.classes,
                ctx.id,
                ctx.attrs,
                ctx.is_root,
                ctx.ancestors,
            );
        }

        let target_comp = &self.components[self.components.len() - 1];
        if !target_comp.matches_element_context(ctx) {
            return false;
        }

        if self.components.len() == 1 {
            return true;
        }

        let ancestor_matches = |anc: &ElementAncestry,
                                idx: usize,
                                comp: &CompoundSelector,
                                ctx: &ElementMatchingContext| {
            // Ancestor contexts now carry the real attribute map so
            // selectors like `form[action] input` match correctly.
            let anc_ctx = ElementMatchingContext::simple(
                &anc.tag,
                &anc.classes,
                anc.id.as_deref(),
                &anc.attrs,
                idx + 1 == ctx.ancestors.len(),
                &ctx.ancestors[idx + 1..],
            );
            comp.matches_element_context(&anc_ctx)
        };

        let mut ancestor_cursor = 0usize;
        for i in (0..self.components.len() - 1).rev() {
            let comp = &self.components[i];
            let comb = self
                .combinators
                .get(i)
                .copied()
                .unwrap_or(Combinator::Descendant);

            match comb {
                Combinator::Child => {
                    let Some(parent) = ctx.ancestors.get(ancestor_cursor) else {
                        return false;
                    };
                    if !ancestor_matches(parent, ancestor_cursor, comp, ctx) {
                        return false;
                    }
                    ancestor_cursor += 1;
                }
                Combinator::Descendant => {
                    let mut found = None;
                    for idx in ancestor_cursor..ctx.ancestors.len() {
                        let anc = &ctx.ancestors[idx];
                        if ancestor_matches(anc, idx, comp, ctx) {
                            found = Some(idx);
                            break;
                        }
                    }
                    let Some(matched_idx) = found else {
                        return false;
                    };
                    ancestor_cursor = matched_idx + 1;
                }
                Combinator::AdjacentSibling => {
                    // The left compound must match the immediately preceding
                    // sibling; siblings share the same ancestor chain.
                    // previous_siblings is document-ordered, so the nearest
                    // prior sibling is the last entry.
                    let Some(prev) = ctx.previous_siblings.last() else {
                        return false;
                    };
                    let prev_ctx = ElementMatchingContext::simple(
                        &prev.tag,
                        &prev.classes,
                        prev.id.as_deref(),
                        &prev.attrs,
                        false,
                        ctx.ancestors,
                    );
                    if !comp.matches_element_context(&prev_ctx) {
                        return false;
                    }
                }
                Combinator::GeneralSibling => {
                    // Any preceding sibling may satisfy the compound.
                    let matched = ctx.previous_siblings.iter().any(|prev| {
                        let prev_ctx = ElementMatchingContext::simple(
                            &prev.tag,
                            &prev.classes,
                            prev.id.as_deref(),
                            &prev.attrs,
                            false,
                            ctx.ancestors,
                        );
                        comp.matches_element_context(&prev_ctx)
                    });
                    if !matched {
                        return false;
                    }
                }
            }
        }

        true
    }

    fn matches_legacy(
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
        if !self.matches_compound_legacy(tag, classes, elem_id, attrs) {
            return false;
        }
        if self.ancestors.is_empty() {
            return true;
        }
        if ancestry.is_empty() {
            return false;
        }

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

    fn matches_compound_legacy(
        &self,
        tag: &str,
        classes: &[String],
        elem_id: Option<&str>,
        attrs: &HashMap<String, String>,
    ) -> bool {
        if let Some(ref sel_tag) = self.tag {
            if sel_tag != "*" && !sel_tag.eq_ignore_ascii_case(tag) {
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
        for (attr_name, attr_val) in &self.attributes {
            let actual = match attrs.get(attr_name) {
                Some(v) => v,
                None => match attrs.get(&attr_name.to_ascii_lowercase()) {
                    Some(v) => v,
                    None => return false,
                },
            };
            if !attr_val.is_empty() && actual != attr_val {
                return false;
            }
        }
        true
    }
}

fn parse_selector_chain(input: &str) -> Option<Selector> {
    let mut components = Vec::new();
    let mut combinators = Vec::new();
    let mut current_token = String::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut bracket_depth: usize = 0;
    let mut paren_depth: usize = 0;

    while i < len {
        let c = chars[i];
        if c == '[' {
            bracket_depth += 1;
            current_token.push(c);
            i += 1;
        } else if c == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            current_token.push(c);
            i += 1;
        } else if c == '(' {
            paren_depth += 1;
            current_token.push(c);
            i += 1;
        } else if c == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            current_token.push(c);
            i += 1;
        } else if bracket_depth == 0
            && paren_depth == 0
            && (c.is_ascii_whitespace() || c == '>' || c == '+' || c == '~')
        {
            if !current_token.trim().is_empty() {
                let (comp, _) = parse_single_compound(current_token.trim());
                if !comp.is_empty() {
                    components.push(comp);
                }
                current_token.clear();
            }

            let mut comb = Combinator::Descendant;
            while i < len
                && (chars[i].is_ascii_whitespace()
                    || chars[i] == '>'
                    || chars[i] == '+'
                    || chars[i] == '~')
            {
                if chars[i] == '>' {
                    comb = Combinator::Child;
                } else if chars[i] == '+' {
                    comb = Combinator::AdjacentSibling;
                } else if chars[i] == '~' {
                    comb = Combinator::GeneralSibling;
                }
                i += 1;
            }
            if !components.is_empty() {
                combinators.push(comb);
            }
        } else {
            current_token.push(c);
            i += 1;
        }
    }

    if !current_token.trim().is_empty() {
        let (comp, _) = parse_single_compound(current_token.trim());
        if !comp.is_empty() {
            components.push(comp);
        }
    }

    if components.is_empty() {
        return None;
    }

    let mut ancestors = Vec::new();
    for (idx, comp) in components[..components.len() - 1].iter().enumerate() {
        let is_direct = combinators.get(idx) == Some(&Combinator::Child);
        ancestors.push(SelectorAncestor {
            tag: comp.tag.clone(),
            class: comp.classes.first().cloned(),
            id: comp.id.clone(),
            direct: is_direct,
        });
    }

    let target = &components[components.len() - 1];
    let is_root = target.is_root;
    let legacy_attrs: Vec<(String, String)> = target
        .attributes
        .iter()
        .map(|a| (a.name.clone(), a.value.clone()))
        .collect();

    Some(Selector {
        tag: target.tag.clone(),
        class: target.classes.first().cloned(),
        id: target.id.clone(),
        attributes: legacy_attrs,
        ancestors,
        is_root,
        components,
        combinators,
    })
}

fn parse_single_compound(input: &str) -> (CompoundSelector, Vec<(String, String)>) {
    let mut comp = CompoundSelector::default();
    let mut legacy_attrs = Vec::new();
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return (comp, legacy_attrs);
    }

    if trimmed == ":root" {
        comp.is_root = true;
        return (comp, legacy_attrs);
    }

    let mut chars = trimmed.chars().peekable();
    let mut current = String::new();
    let mut state = 'T';
    let mut paren_depth: usize = 0;

    while let Some(c) = chars.next() {
        if c == '(' {
            paren_depth += 1;
            current.push(c);
        } else if c == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            current.push(c);
        } else if paren_depth > 0 {
            current.push(c);
        } else {
            match c {
                '[' => {
                    flush_compound_token(&mut comp, state, &current);
                    current.clear();
                    state = ' ';

                    let mut attr_content = String::new();
                    for inner in chars.by_ref() {
                        if inner == ']' {
                            break;
                        }
                        attr_content.push(inner);
                    }
                    if let Some(attr) = parse_attr_selector(&attr_content) {
                        legacy_attrs.push((attr.name.clone(), attr.value.clone()));
                        comp.attributes.push(attr);
                    }
                }
                '.' | '#' | ':' => {
                    flush_compound_token(&mut comp, state, &current);
                    current.clear();
                    if c == ':' && chars.peek() == Some(&':') {
                        chars.next();
                        state = 'E';
                    } else {
                        state = c;
                    }
                }
                _ => {
                    current.push(c);
                }
            }
        }
    }

    flush_compound_token(&mut comp, state, &current);
    (comp, legacy_attrs)
}

fn flush_compound_token(comp: &mut CompoundSelector, state: char, token: &str) {
    let token = token.trim();
    if token.is_empty() {
        return;
    }
    match state {
        'T' => {
            if token == ":root" {
                comp.is_root = true;
            } else if let Some((_, tag)) = token.split_once('|') {
                comp.tag = Some(tag.to_ascii_lowercase());
            } else {
                comp.tag = Some(token.to_ascii_lowercase());
            }
        }
        '.' => {
            comp.classes.push(token.to_string());
        }
        '#' => {
            comp.id = Some(token.to_string());
        }
        ':' => {
            if let Some(pc) = parse_pseudo_class(token) {
                if pc == PseudoClass::Root {
                    comp.is_root = true;
                }
                comp.pseudo_classes.push(pc);
            }
        }
        'E' => {
            if let Some(pe) = parse_pseudo_element(token) {
                comp.pseudo_element = Some(pe);
            }
        }
        _ => {}
    }
}

fn parse_attr_selector(content: &str) -> Option<AttributeSelector> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    let op_signatures = [
        ("~=", AttributeMatch::Includes),
        ("|=", AttributeMatch::DashMatch),
        ("^=", AttributeMatch::Prefix),
        ("$=", AttributeMatch::Suffix),
        ("*=", AttributeMatch::Substring),
        ("=", AttributeMatch::Exact),
    ];

    let mut found_op = None;
    for (sig, match_type) in op_signatures {
        if let Some(pos) = trimmed.find(sig) {
            found_op = Some((pos, sig.len(), match_type));
            break;
        }
    }

    let (name, op, val, case_sens) = if let Some((pos, len, match_type)) = found_op {
        let name = trimmed[..pos].trim().to_string();
        let mut rest = trimmed[pos + len..].trim();
        let mut case_sens = CaseSensitivity::Default;

        if rest.ends_with(" i") || rest.ends_with(" I") {
            case_sens = CaseSensitivity::CaseInsensitive;
            rest = rest[..rest.len() - 2].trim();
        } else if rest.ends_with(" s") || rest.ends_with(" S") {
            case_sens = CaseSensitivity::CaseSensitive;
            rest = rest[..rest.len() - 2].trim();
        }

        let val = rest.trim_matches('"').trim_matches('\'').to_string();
        (name, match_type, val, case_sens)
    } else {
        (
            trimmed.to_string(),
            AttributeMatch::Presence,
            String::new(),
            CaseSensitivity::Default,
        )
    };

    if name.is_empty() {
        None
    } else {
        Some(AttributeSelector {
            name,
            operator: op,
            value: val,
            case_sensitivity: case_sens,
        })
    }
}

fn parse_pseudo_class(token: &str) -> Option<PseudoClass> {
    let lower = token.to_ascii_lowercase();
    if lower == "root" {
        return Some(PseudoClass::Root);
    }
    if lower == "empty" {
        return Some(PseudoClass::Empty);
    }
    if lower == "first-child" {
        return Some(PseudoClass::FirstChild);
    }
    if lower == "last-child" {
        return Some(PseudoClass::LastChild);
    }
    if lower == "only-child" {
        return Some(PseudoClass::OnlyChild);
    }
    if lower == "first-of-type" {
        return Some(PseudoClass::FirstOfType);
    }
    if lower == "last-of-type" {
        return Some(PseudoClass::LastOfType);
    }
    if lower == "only-of-type" {
        return Some(PseudoClass::OnlyOfType);
    }
    if lower == "hover" {
        return Some(PseudoClass::Hover);
    }
    if lower == "active" {
        return Some(PseudoClass::Active);
    }
    if lower == "focus" {
        return Some(PseudoClass::Focus);
    }
    if lower == "focus-visible" {
        return Some(PseudoClass::FocusVisible);
    }
    if lower == "focus-within" {
        return Some(PseudoClass::FocusWithin);
    }
    if lower == "enabled" {
        return Some(PseudoClass::Enabled);
    }
    if lower == "disabled" {
        return Some(PseudoClass::Disabled);
    }
    if lower == "checked" {
        return Some(PseudoClass::Checked);
    }
    if lower == "indeterminate" {
        return Some(PseudoClass::Indeterminate);
    }
    if lower == "link" {
        return Some(PseudoClass::Link);
    }
    if lower == "visited" {
        return Some(PseudoClass::Visited);
    }
    if lower == "target" {
        return Some(PseudoClass::Target);
    }
    if lower == "valid" {
        return Some(PseudoClass::Valid);
    }
    if lower == "invalid" {
        return Some(PseudoClass::Invalid);
    }
    if lower == "required" {
        return Some(PseudoClass::Required);
    }
    if lower == "optional" {
        return Some(PseudoClass::Optional);
    }
    if lower == "read-only" {
        return Some(PseudoClass::ReadOnly);
    }
    if lower == "read-write" {
        return Some(PseudoClass::ReadWrite);
    }

    if lower == "before" {
        return Some(PseudoClass::Custom("before".to_string()));
    }
    if lower == "after" {
        return Some(PseudoClass::Custom("after".to_string()));
    }

    if let Some(open) = token.find('(') {
        if token.ends_with(')') {
            let func_name = token[..open].trim().to_ascii_lowercase();
            let inner = token[open + 1..token.len() - 1].trim();
            match func_name.as_str() {
                "not" => {
                    let list = parse_selector_list(inner);
                    return Some(PseudoClass::Not(list));
                }
                "is" | "matches" => {
                    let list = parse_selector_list(inner);
                    return Some(PseudoClass::Is(list));
                }
                "where" => {
                    let list = parse_selector_list(inner);
                    return Some(PseudoClass::Where(list));
                }
                "has" => {
                    let list = parse_selector_list(inner);
                    return Some(PseudoClass::Has(list));
                }
                "lang" => {
                    return Some(PseudoClass::Lang(inner.to_string()));
                }
                "nth-child" => {
                    let (a, b) = parse_an_plus_b(inner);
                    return Some(PseudoClass::NthChild {
                        a,
                        b,
                        of_selector: None,
                    });
                }
                "nth-last-child" => {
                    let (a, b) = parse_an_plus_b(inner);
                    return Some(PseudoClass::NthLastChild {
                        a,
                        b,
                        of_selector: None,
                    });
                }
                "nth-of-type" => {
                    let (a, b) = parse_an_plus_b(inner);
                    return Some(PseudoClass::NthOfType { a, b });
                }
                "nth-last-of-type" => {
                    let (a, b) = parse_an_plus_b(inner);
                    return Some(PseudoClass::NthLastOfType { a, b });
                }
                _ => {}
            }
        }
    }

    Some(PseudoClass::Custom(token.to_string()))
}

fn parse_pseudo_element(token: &str) -> Option<PseudoElement> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "before" => Some(PseudoElement::Before),
        "after" => Some(PseudoElement::After),
        "placeholder" => Some(PseudoElement::Placeholder),
        "first-letter" => Some(PseudoElement::FirstLetter),
        "first-line" => Some(PseudoElement::FirstLine),
        "marker" => Some(PseudoElement::Marker),
        "selection" => Some(PseudoElement::Selection),
        _ => Some(PseudoElement::Custom(token.to_string())),
    }
}

fn parse_an_plus_b(expr: &str) -> (i32, i32) {
    let expr = expr.trim().to_ascii_lowercase().replace(' ', "");
    if expr == "odd" {
        return (2, 1);
    }
    if expr == "even" {
        return (2, 0);
    }
    if let Ok(num) = expr.parse::<i32>() {
        return (0, num);
    }
    if let Some((a_str, b_str)) = expr.split_once('n') {
        let a = if a_str.is_empty() || a_str == "+" {
            1
        } else if a_str == "-" {
            -1
        } else {
            a_str.parse::<i32>().unwrap_or(1)
        };

        let b = if b_str.is_empty() {
            0
        } else {
            b_str.parse::<i32>().unwrap_or(0)
        };
        (a, b)
    } else {
        (0, 1)
    }
}

/// Nesting budget for functional pseudo-classes (:not/:is/:where/:has).
/// The parse cycle parse_selector_list -> Selector::parse ->
/// parse_selector_chain -> parse_single_compound -> parse_pseudo_class
/// re-enters this function once per nesting level, so an attacker
/// stylesheet with thousands of nested pseudo-classes would otherwise
/// overflow the stack.
const MAX_SELECTOR_NESTING_DEPTH: usize = 32;

fn parse_selector_list(input: &str) -> Vec<Selector> {
    use std::cell::Cell;
    thread_local! {
        static NESTING: Cell<usize> = const { Cell::new(0) };
    }
    struct NestingGuard;
    impl Drop for NestingGuard {
        fn drop(&mut self) {
            NESTING.with(|n| n.set(n.get().saturating_sub(1)));
        }
    }
    let allowed = NESTING.with(|n| {
        let current = n.get();
        if current >= MAX_SELECTOR_NESTING_DEPTH {
            false
        } else {
            n.set(current + 1);
            true
        }
    });
    if !allowed {
        // Depth budget exhausted: fail closed by yielding no selectors so
        // the rule simply does not match.
        return Vec::new();
    }
    let _guard = NestingGuard;

    let mut list = Vec::new();
    let mut current = String::new();
    let mut paren_depth: usize = 0;
    let mut bracket_depth: usize = 0;

    for c in input.chars() {
        match c {
            '(' => {
                paren_depth += 1;
                current.push(c);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(c);
            }
            '[' => {
                bracket_depth += 1;
                current.push(c);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(c);
            }
            ',' if paren_depth == 0 && bracket_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    list.push(Selector::parse(trimmed));
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        list.push(Selector::parse(trimmed));
    }
    list
}

/// Ancestry element on the hierarchy chain (nearest ancestor first).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ElementAncestry {
    pub tag: String,
    pub classes: Vec<String>,
    pub id: Option<String>,
    /// Real attribute map so selectors like `form[action] input` can match
    /// ancestors. Empty for hand-built summaries.
    #[serde(default)]
    pub attrs: HashMap<String, String>,
}

/// Real sibling-position facts computed by the style traversal. Without them
/// structural pseudo-classes (`:first-child`, `:nth-child(2)`, ...) evaluate
/// against fabricated defaults and match every element.
///
/// `previous_siblings` lists PRIOR siblings in DOCUMENT ORDER (the
/// immediately preceding sibling is the LAST entry) and borrows the
/// traversal's buffer so hot cascades never clone per element.
#[derive(Debug, Clone, Default)]
pub struct SiblingContext<'a> {
    /// 1-based position among the parent's children.
    pub index_in_parent: usize,
    pub siblings_after: usize,
    pub total_siblings: usize,
    /// Same four facts restricted to siblings sharing this tag.
    pub type_index_in_parent: usize,
    pub total_type_siblings: usize,
    /// Prior siblings in document order, capped by the traversal.
    pub previous_siblings: &'a [ElementAncestry],
}

/// A parsed CSS declaration.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Declaration {
    pub property: String,
    pub value: String,
    #[serde(default)]
    pub important: bool,
    #[serde(default)]
    pub origin: CssOrigin,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub layer_order: Option<usize>,
    #[serde(default)]
    pub specificity: (u32, u32, u32),
    #[serde(default)]
    pub source_order: usize,
}

/// Position mode for layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PositionMode {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
}

/// Axis-aligned 2D transform values.
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
    let value = value.trim();
    if let Some(px) = value.strip_suffix("px") {
        px.trim()
            .parse::<f64>()
            .ok()
            .map(|v| v.clamp(-1_000_000.0, 1_000_000.0))
    } else {
        value
            .parse::<f64>()
            .ok()
            .map(|v| v.clamp(-1_000_000.0, 1_000_000.0))
    }
}

/// `line-height` stored per CSS semantics: unitless numbers and percentages
/// scale with the element's font size, while px/pt lengths are absolute.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LineHeight {
    /// Unitless number or percentage — a multiplier of the font size.
    Multiplier(f64),
    /// Absolute length already normalized to CSS pixels.
    Absolute(f64),
}

/// Typed CSS Units supporting pixels, percentages, font-relative, viewport-relative, and calc expressions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CssUnit {
    Pixels(f64),
    Percent(f64),
    Em(f64),
    Rem(f64),
    Vw(f64),
    Vh(f64),
    Vmin(f64),
    Vmax(f64),
    Pt(f64),
    Auto,
    Calc(String),
}

impl CssUnit {
    pub fn to_pixels(&self, parent_size: f64, root_size: f64) -> f64 {
        self.to_pixels_with_viewport(parent_size, root_size, 1024.0, 768.0)
    }

    pub fn to_pixels_with_viewport(
        &self,
        parent_size: f64,
        root_size: f64,
        vw: f64,
        vh: f64,
    ) -> f64 {
        match self {
            CssUnit::Pixels(px) => *px,
            CssUnit::Percent(pct) => parent_size * pct / 100.0,
            CssUnit::Em(em) => parent_size * em,
            CssUnit::Rem(rem) => root_size * rem,
            CssUnit::Vw(v) => vw * v / 100.0,
            CssUnit::Vh(v) => vh * v / 100.0,
            CssUnit::Vmin(v) => vw.min(vh) * v / 100.0,
            CssUnit::Vmax(v) => vw.max(vh) * v / 100.0,
            CssUnit::Pt(pt) => pt * 96.0 / 72.0,
            CssUnit::Auto => 0.0,
            CssUnit::Calc(expr) => {
                eval_math_expression(expr, parent_size, root_size, vw, vh, &HashMap::new())
                    .unwrap_or(0.0)
            }
        }
    }

    pub fn parse(value: &str) -> Option<CssUnit> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if value.eq_ignore_ascii_case("auto")
            || value.eq_ignore_ascii_case("inherit")
            || value.eq_ignore_ascii_case("initial")
            || value.eq_ignore_ascii_case("unset")
        {
            return Some(CssUnit::Auto);
        }

        if value.starts_with("calc(")
            || value.starts_with("min(")
            || value.starts_with("max(")
            || value.starts_with("clamp(")
        {
            return Some(CssUnit::Calc(value.to_string()));
        }

        if let Some(px_val) = value.strip_suffix("px") {
            px_val.trim().parse::<f64>().ok().map(CssUnit::Pixels)
        } else if let Some(pct_val) = value.strip_suffix('%') {
            pct_val.trim().parse::<f64>().ok().map(CssUnit::Percent)
        } else if let Some(rem_val) = value.strip_suffix("rem") {
            rem_val.trim().parse::<f64>().ok().map(CssUnit::Rem)
        } else if let Some(em_val) = value.strip_suffix("em") {
            em_val.trim().parse::<f64>().ok().map(CssUnit::Em)
        } else if let Some(vw_val) = value.strip_suffix("vw") {
            vw_val.trim().parse::<f64>().ok().map(CssUnit::Vw)
        } else if let Some(vh_val) = value.strip_suffix("vh") {
            vh_val.trim().parse::<f64>().ok().map(CssUnit::Vh)
        } else if let Some(vmin_val) = value.strip_suffix("vmin") {
            vmin_val.trim().parse::<f64>().ok().map(CssUnit::Vmin)
        } else if let Some(vmax_val) = value.strip_suffix("vmax") {
            vmax_val.trim().parse::<f64>().ok().map(CssUnit::Vmax)
        } else if let Some(pt_val) = value.strip_suffix("pt") {
            pt_val.trim().parse::<f64>().ok().map(CssUnit::Pt)
        } else if let Some(dvw_val) = value
            .strip_suffix("dvw")
            .or_else(|| value.strip_suffix("svw"))
            .or_else(|| value.strip_suffix("lvw"))
        {
            dvw_val.trim().parse::<f64>().ok().map(CssUnit::Vw)
        } else if let Some(dvh_val) = value
            .strip_suffix("dvh")
            .or_else(|| value.strip_suffix("svh"))
            .or_else(|| value.strip_suffix("lvh"))
        {
            dvh_val.trim().parse::<f64>().ok().map(CssUnit::Vh)
        } else {
            value.parse::<f64>().ok().map(CssUnit::Pixels)
        }
    }
}

/// Fully computed CSS style for a DOM element.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComputedStyle {
    // Colors
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub background_image: Option<String>,
    pub background_repeat: Option<String>,
    pub background_position: Option<String>,
    pub background_size: Option<String>,

    // Font & Typography
    pub font_family: Option<String>,
    pub font_size: Option<CssUnit>,
    pub font_weight: Option<u16>,
    pub font_style: Option<String>,
    pub text_align: Option<String>,
    pub text_decoration: Option<String>,
    pub text_transform: Option<String>,
    pub line_height: Option<LineHeight>,
    pub letter_spacing: Option<CssUnit>,
    pub word_spacing: Option<CssUnit>,
    pub white_space: Option<String>,
    pub word_break: Option<String>,

    // Box Model & Sizing
    pub display: Option<String>,
    pub box_sizing: Option<String>,
    pub width: Option<CssUnit>,
    pub height: Option<CssUnit>,
    pub min_width: Option<CssUnit>,
    pub max_width: Option<CssUnit>,
    pub min_height: Option<CssUnit>,
    pub max_height: Option<CssUnit>,
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
    pub border_top_width: Option<CssUnit>,
    pub border_right_width: Option<CssUnit>,
    pub border_bottom_width: Option<CssUnit>,
    pub border_left_width: Option<CssUnit>,
    pub border_radius: Option<CssUnit>,

    // Flexbox
    pub flex_direction: Option<String>,
    pub flex_wrap: Option<String>,
    pub justify_content: Option<String>,
    pub align_items: Option<String>,
    pub align_content: Option<String>,
    pub align_self: Option<String>,
    pub flex_grow: f64,
    pub flex_shrink: f64,
    pub flex_basis: Option<CssUnit>,
    pub order: i32,
    pub gap: Option<CssUnit>,
    pub row_gap: Option<CssUnit>,
    pub column_gap: Option<CssUnit>,

    // Grid
    pub grid_template_columns: Option<String>,
    pub grid_template_rows: Option<String>,
    pub grid_template_areas: Option<String>,
    pub grid_column_span: usize,
    pub grid_row_span: usize,
    pub grid_column_start: Option<String>,
    pub grid_column_end: Option<String>,
    pub grid_row_start: Option<String>,
    pub grid_row_end: Option<String>,

    // Positioning & Layout
    pub position: PositionMode,
    pub top: Option<CssUnit>,
    pub right: Option<CssUnit>,
    pub bottom: Option<CssUnit>,
    pub left: Option<CssUnit>,
    pub z_index: i32,
    pub float: Option<String>,
    pub clear: Option<String>,

    // Visual & Effects
    pub overflow: Option<String>,
    pub overflow_x: Option<String>,
    pub overflow_y: Option<String>,
    pub opacity: Option<f64>,
    pub visibility: Option<String>,
    pub cursor: Option<String>,
    pub pointer_events: Option<String>,
    pub user_select: Option<String>,
    pub box_shadow: Option<String>,
    pub transform: Transform2D,
    pub transition: Option<String>,
    pub animation_name: Option<String>,
    pub animation_duration_ms: u64,
    pub animation_iterations: u16,

    // Custom Properties (CSS variables)
    pub custom_properties: HashMap<String, String>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            color: Some("#000000".to_string()),
            background_color: None,
            background_image: None,
            background_repeat: None,
            background_position: None,
            background_size: None,
            font_family: Some("sans-serif".to_string()),
            font_size: Some(CssUnit::Pixels(16.0)),
            font_weight: Some(400),
            font_style: None,
            text_align: Some("left".to_string()),
            text_decoration: None,
            text_transform: None,
            line_height: Some(LineHeight::Multiplier(1.4)),
            letter_spacing: None,
            word_spacing: None,
            white_space: None,
            word_break: None,
            display: Some("block".to_string()),
            box_sizing: Some("content-box".to_string()),
            width: None,
            height: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
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
            border_top_width: None,
            border_right_width: None,
            border_bottom_width: None,
            border_left_width: None,
            border_radius: None,
            flex_direction: None,
            flex_wrap: None,
            justify_content: None,
            align_items: None,
            align_content: None,
            align_self: None,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: None,
            order: 0,
            gap: None,
            row_gap: None,
            column_gap: None,
            grid_template_columns: None,
            grid_template_rows: None,
            grid_template_areas: None,
            grid_column_span: 1,
            grid_row_span: 1,
            grid_column_start: None,
            grid_column_end: None,
            grid_row_start: None,
            grid_row_end: None,
            position: PositionMode::Static,
            top: None,
            right: None,
            bottom: None,
            left: None,
            z_index: 0,
            float: None,
            clear: None,
            overflow: Some("visible".to_string()),
            overflow_x: None,
            overflow_y: None,
            opacity: Some(1.0),
            visibility: Some("visible".to_string()),
            cursor: None,
            pointer_events: None,
            user_select: None,
            box_shadow: None,
            transform: Transform2D::default(),
            transition: None,
            animation_name: None,
            animation_duration_ms: 0,
            animation_iterations: 1,
            custom_properties: HashMap::new(),
        }
    }
}

const MAX_CUSTOM_PROPERTIES: usize = 256;

impl ComputedStyle {
    pub fn apply_declaration(&mut self, decl: &Declaration) {
        self.apply_declaration_resolved(decl, &HashMap::new())
    }

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
                    self.custom_properties.insert(property, value);
                }
            }
            return;
        }

        let val_trimmed = decl.value.trim().to_ascii_lowercase();
        if val_trimmed == "initial" || val_trimmed == "unset" || val_trimmed == "inherit" {
            self.apply_css_wide_keyword(&property, &val_trimmed);
            return;
        }

        let resolved = resolve_var_value(&decl.value, customs);
        let mut resolved_decl = decl.clone();
        if let Some(value) = resolved {
            if value.is_empty() {
                self.apply_css_wide_keyword(&property, "unset");
                return;
            }
            resolved_decl.value = value;
        }
        self.apply_declaration_plain(&resolved_decl);
    }

    fn apply_css_wide_keyword(&mut self, property: &str, keyword: &str) {
        let is_inherited = is_property_inherited(property);
        let treat_as_inherit = keyword == "inherit" || (keyword == "unset" && is_inherited);

        if treat_as_inherit {
            return;
        }

        let default_style = ComputedStyle::default();
        match property {
            "color" => self.color = default_style.color,
            "background-color" | "background" => self.background_color = None,
            "font-family" => self.font_family = default_style.font_family,
            "font-size" => self.font_size = default_style.font_size,
            "font-weight" => self.font_weight = default_style.font_weight,
            "font-style" => self.font_style = None,
            "text-align" => self.text_align = default_style.text_align,
            "line-height" => self.line_height = default_style.line_height,
            "display" => self.display = default_style.display,
            "width" => self.width = None,
            "height" => self.height = None,
            "margin-top" => self.margin_top = None,
            "margin-right" => self.margin_right = None,
            "margin-bottom" => self.margin_bottom = None,
            "margin-left" => self.margin_left = None,
            "padding-top" => self.padding_top = None,
            "padding-right" => self.padding_right = None,
            "padding-bottom" => self.padding_bottom = None,
            "padding-left" => self.padding_left = None,
            "border-width" => self.border_width = None,
            "border-style" => self.border_style = None,
            "border-color" => self.border_color = None,
            "position" => self.position = PositionMode::Static,
            "top" => self.top = None,
            "right" => self.right = None,
            "bottom" => self.bottom = None,
            "left" => self.left = None,
            "z-index" => self.z_index = 0,
            "opacity" => self.opacity = Some(1.0),
            "overflow" => self.overflow = Some("visible".to_string()),
            _ => {}
        }
    }

    fn apply_declaration_plain(&mut self, decl: &Declaration) {
        let prop = decl.property.as_str();
        let val = decl.value.trim();

        match prop {
            "color" => self.color = Some(val.to_string()),
            "background-color" => self.background_color = Some(val.to_string()),
            "background" => apply_background_shorthand(self, val),
            "background-image" => self.background_image = Some(val.to_string()),
            "background-repeat" => self.background_repeat = Some(val.to_string()),
            "background-position" => self.background_position = Some(val.to_string()),
            "background-size" => self.background_size = Some(val.to_string()),

            "font" => apply_font_shorthand(self, val),
            "font-family" => self.font_family = Some(val.to_string()),
            "font-size" => self.font_size = CssUnit::parse(val),
            "font-weight" => {
                self.font_weight = match val.to_ascii_lowercase().as_str() {
                    "normal" | "400" => Some(400),
                    "bold" | "700" => Some(700),
                    "lighter" => Some(300),
                    "bolder" => Some(900),
                    _ => val.parse::<u16>().ok(),
                };
            }
            "font-style" => self.font_style = Some(val.to_string()),
            "text-align" => self.text_align = Some(val.to_string()),
            "text-decoration" | "text-decoration-line" => {
                self.text_decoration = Some(val.to_string())
            }
            "text-transform" => self.text_transform = Some(val.to_string()),
            "line-height" => self.line_height = parse_line_height(val),
            "letter-spacing" => self.letter_spacing = CssUnit::parse(val),
            "word-spacing" => self.word_spacing = CssUnit::parse(val),
            "white-space" => self.white_space = Some(val.to_string()),
            "word-break" => self.word_break = Some(val.to_string()),

            "display" => self.display = Some(val.to_string()),
            "box-sizing" => self.box_sizing = Some(val.to_string()),
            "width" => self.width = CssUnit::parse(val),
            "height" => self.height = CssUnit::parse(val),
            "min-width" => self.min_width = CssUnit::parse(val),
            "max-width" => self.max_width = CssUnit::parse(val),
            "min-height" => self.min_height = CssUnit::parse(val),
            "max-height" => self.max_height = CssUnit::parse(val),

            "margin" => apply_four_sided_shorthand(
                val,
                &mut self.margin_top,
                &mut self.margin_right,
                &mut self.margin_bottom,
                &mut self.margin_left,
            ),
            "margin-top" => self.margin_top = CssUnit::parse(val),
            "margin-right" => self.margin_right = CssUnit::parse(val),
            "margin-bottom" => self.margin_bottom = CssUnit::parse(val),
            "margin-left" => self.margin_left = CssUnit::parse(val),
            "margin-inline" => {
                let u = CssUnit::parse(val);
                self.margin_left = u.clone();
                self.margin_right = u;
            }
            "margin-block" => {
                let u = CssUnit::parse(val);
                self.margin_top = u.clone();
                self.margin_bottom = u;
            }

            "padding" => apply_four_sided_shorthand(
                val,
                &mut self.padding_top,
                &mut self.padding_right,
                &mut self.padding_bottom,
                &mut self.padding_left,
            ),
            "padding-top" => self.padding_top = CssUnit::parse(val),
            "padding-right" => self.padding_right = CssUnit::parse(val),
            "padding-bottom" => self.padding_bottom = CssUnit::parse(val),
            "padding-left" => self.padding_left = CssUnit::parse(val),
            "padding-inline" => {
                let u = CssUnit::parse(val);
                self.padding_left = u.clone();
                self.padding_right = u;
            }
            "padding-block" => {
                let u = CssUnit::parse(val);
                self.padding_top = u.clone();
                self.padding_bottom = u;
            }

            "border" => apply_border_shorthand(self, val),
            "border-width" => {
                self.border_width = CssUnit::parse(val.split_whitespace().next().unwrap_or(val))
            }
            "border-style" => self.border_style = Some(val.to_string()),
            "border-color" => self.border_color = Some(val.to_string()),
            "border-radius" => {
                self.border_radius = CssUnit::parse(val.split_whitespace().next().unwrap_or(val))
            }
            "border-top" => apply_border_side(
                val,
                &mut self.border_top_width,
                &mut self.border_style,
                &mut self.border_color,
            ),
            "border-right" => apply_border_side(
                val,
                &mut self.border_right_width,
                &mut self.border_style,
                &mut self.border_color,
            ),
            "border-bottom" => apply_border_side(
                val,
                &mut self.border_bottom_width,
                &mut self.border_style,
                &mut self.border_color,
            ),
            "border-left" => apply_border_side(
                val,
                &mut self.border_left_width,
                &mut self.border_style,
                &mut self.border_color,
            ),

            "flex" => apply_flex_shorthand(self, val),
            "flex-direction" => self.flex_direction = Some(val.to_string()),
            "flex-wrap" => self.flex_wrap = Some(val.to_string()),
            "flex-flow" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                if let Some(dir) = parts.first() {
                    self.flex_direction = Some((*dir).to_string());
                }
                if let Some(wrap) = parts.get(1) {
                    self.flex_wrap = Some((*wrap).to_string());
                }
            }
            "flex-grow" => self.flex_grow = val.parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0),
            "flex-shrink" => self.flex_shrink = val.parse::<f64>().unwrap_or(1.0).clamp(0.0, 100.0),
            "flex-basis" => self.flex_basis = CssUnit::parse(val),
            "justify-content" => self.justify_content = Some(val.to_string()),
            "align-items" => self.align_items = Some(val.to_string()),
            "align-content" => self.align_content = Some(val.to_string()),
            "align-self" => self.align_self = Some(val.to_string()),
            "order" => self.order = val.parse::<i32>().unwrap_or(0).clamp(-10_000, 10_000),
            "gap" => {
                let u = CssUnit::parse(val);
                self.gap = u.clone();
                self.row_gap = u.clone();
                self.column_gap = u;
            }
            "row-gap" => self.row_gap = CssUnit::parse(val),
            "column-gap" => self.column_gap = CssUnit::parse(val),

            "grid-template-columns" => self.grid_template_columns = Some(val.to_string()),
            "grid-template-rows" => self.grid_template_rows = Some(val.to_string()),
            "grid-template-areas" => self.grid_template_areas = Some(val.to_string()),
            "grid-column" => self.grid_column_span = parse_grid_span(val),
            "grid-row" => self.grid_row_span = parse_grid_span(val),

            "position" => {
                self.position = match val.to_ascii_lowercase().as_str() {
                    "relative" | "sticky" => PositionMode::Relative,
                    "absolute" => PositionMode::Absolute,
                    "fixed" => PositionMode::Fixed,
                    _ => PositionMode::Static,
                };
            }
            "top" => self.top = CssUnit::parse(val),
            "right" => self.right = CssUnit::parse(val),
            "bottom" => self.bottom = CssUnit::parse(val),
            "left" => self.left = CssUnit::parse(val),
            "inset" => apply_four_sided_shorthand(
                val,
                &mut self.top,
                &mut self.right,
                &mut self.bottom,
                &mut self.left,
            ),
            "z-index" => self.z_index = val.parse::<i32>().unwrap_or(0).clamp(-10_000, 10_000),
            "float" => self.float = Some(val.to_string()),
            "clear" => self.clear = Some(val.to_string()),

            "overflow" => {
                self.overflow = Some(val.to_string());
                self.overflow_x = Some(val.to_string());
                self.overflow_y = Some(val.to_string());
            }
            "overflow-x" => self.overflow_x = Some(val.to_string()),
            "overflow-y" => self.overflow_y = Some(val.to_string()),
            "opacity" => self.opacity = val.parse::<f64>().ok().map(|v| v.clamp(0.0, 1.0)),
            "visibility" => self.visibility = Some(val.to_string()),
            "cursor" => self.cursor = Some(val.to_string()),
            "pointer-events" => self.pointer_events = Some(val.to_string()),
            "user-select" => self.user_select = Some(val.to_string()),
            "box-shadow" => self.box_shadow = Some(val.to_string()),
            "transform" => {
                if let Some(t) = Transform2D::parse(val) {
                    self.transform = t;
                }
            }
            "transition" | "transition-property" | "transition-duration" => {
                self.transition = Some(val.to_string());
            }
            "animation" => apply_animation_shorthand(self, val),
            "animation-name" => self.animation_name = Some(val.to_string()),
            "animation-duration" => self.animation_duration_ms = parse_duration_ms(val),
            "animation-iteration-count" => {
                self.animation_iterations = parse_animation_iterations(val)
            }

            _ => {
                record_unsupported_property(prop);
            }
        }
    }
}

fn is_property_inherited(property: &str) -> bool {
    matches!(
        property,
        "color"
            | "font-family"
            | "font-size"
            | "font-weight"
            | "font-style"
            | "font-variant"
            | "line-height"
            | "letter-spacing"
            | "word-spacing"
            | "text-align"
            | "text-indent"
            | "text-transform"
            | "white-space"
            | "visibility"
            | "cursor"
            | "quotes"
            | "list-style"
            | "list-style-type"
            | "list-style-position"
            | "list-style-image"
            | "border-collapse"
            | "border-spacing"
            | "direction"
    )
}

fn parse_line_height(val: &str) -> Option<LineHeight> {
    if let Ok(num) = val.parse::<f64>() {
        return Some(LineHeight::Multiplier(num));
    }
    if let Some(px) = val.strip_suffix("px") {
        return px.trim().parse::<f64>().ok().map(LineHeight::Absolute);
    }
    if let Some(pt) = val.strip_suffix("pt") {
        return pt
            .trim()
            .parse::<f64>()
            .ok()
            .map(|p| LineHeight::Absolute(p * 96.0 / 72.0));
    }
    if let Some(em) = val.strip_suffix("em") {
        // em resolves against the element's font size, like a multiplier.
        return em.trim().parse::<f64>().ok().map(LineHeight::Multiplier);
    }
    if let Some(pct) = val.strip_suffix('%') {
        return pct
            .trim()
            .parse::<f64>()
            .ok()
            .map(|p| LineHeight::Multiplier(p / 100.0));
    }
    None
}

/// Resolve a computed `line-height` to pixels for a given font size.
/// Unitless numbers and percentages scale with the font; px/pt lengths are
/// absolute and must NOT be multiplied by the element's font size.
pub fn line_height_px(line_height: Option<LineHeight>, font_size: f64) -> f64 {
    match line_height {
        Some(LineHeight::Multiplier(m)) => (m * font_size).max(0.0),
        Some(LineHeight::Absolute(px)) => px.max(0.0),
        None => font_size * 1.4,
    }
}

fn apply_four_sided_shorthand(
    val: &str,
    top: &mut Option<CssUnit>,
    right: &mut Option<CssUnit>,
    bottom: &mut Option<CssUnit>,
    left: &mut Option<CssUnit>,
) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    match parts.len() {
        1 => {
            let u = CssUnit::parse(parts[0]);
            *top = u.clone();
            *right = u.clone();
            *bottom = u.clone();
            *left = u;
        }
        2 => {
            let u_tb = CssUnit::parse(parts[0]);
            let u_lr = CssUnit::parse(parts[1]);
            *top = u_tb.clone();
            *right = u_lr.clone();
            *bottom = u_tb;
            *left = u_lr;
        }
        3 => {
            let u_t = CssUnit::parse(parts[0]);
            let u_lr = CssUnit::parse(parts[1]);
            let u_b = CssUnit::parse(parts[2]);
            *top = u_t;
            *right = u_lr.clone();
            *bottom = u_b;
            *left = u_lr;
        }
        4 => {
            *top = CssUnit::parse(parts[0]);
            *right = CssUnit::parse(parts[1]);
            *bottom = CssUnit::parse(parts[2]);
            *left = CssUnit::parse(parts[3]);
        }
        _ => {}
    }
}

fn apply_border_shorthand(style: &mut ComputedStyle, val: &str) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if let Some(u) = CssUnit::parse(part) {
            style.border_width = Some(u);
        } else if matches!(
            part.to_ascii_lowercase().as_str(),
            "none"
                | "solid"
                | "dashed"
                | "dotted"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            style.border_style = Some(part.to_string());
        } else if parse_css_color(part).is_some() {
            style.border_color = Some(part.to_string());
        }
    }
}

fn apply_border_side(
    val: &str,
    side_width: &mut Option<CssUnit>,
    border_style: &mut Option<String>,
    border_color: &mut Option<String>,
) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if let Some(u) = CssUnit::parse(part) {
            *side_width = Some(u);
        } else if matches!(
            part.to_ascii_lowercase().as_str(),
            "none" | "solid" | "dashed" | "dotted" | "double"
        ) {
            *border_style = Some(part.to_string());
        } else if parse_css_color(part).is_some() {
            *border_color = Some(part.to_string());
        }
    }
}

fn apply_background_shorthand(style: &mut ComputedStyle, val: &str) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if parse_css_color(part).is_some() {
            style.background_color = Some(part.to_string());
        } else if part.starts_with("url(")
            || part.starts_with("linear-gradient(")
            || part.starts_with("radial-gradient(")
        {
            style.background_image = Some(part.to_string());
        } else if matches!(
            part.to_ascii_lowercase().as_str(),
            "no-repeat" | "repeat" | "repeat-x" | "repeat-y"
        ) {
            style.background_repeat = Some(part.to_string());
        }
    }
}

fn apply_font_shorthand(style: &mut ComputedStyle, val: &str) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if let Some(u) = CssUnit::parse(part) {
            style.font_size = Some(u);
        } else if part == "bold" || part == "700" {
            style.font_weight = Some(700);
        } else if part == "italic" {
            style.font_style = Some("italic".to_string());
        } else if !part.contains('/') && !part.parse::<f64>().is_ok() {
            style.font_family = Some(part.to_string());
        }
    }
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

fn resolve_var_value(value: &str, customs: &HashMap<String, String>) -> Option<String> {
    if !value.contains("var(") {
        return None;
    }
    let mut visited = HashSet::new();
    resolve_var_recursive(value, customs, &mut visited, 0)
}

fn resolve_var_recursive(
    value: &str,
    customs: &HashMap<String, String>,
    visited: &mut HashSet<String>,
    depth: usize,
) -> Option<String> {
    if depth > 16 {
        return None;
    }
    if !value.contains("var(") {
        return Some(value.to_string());
    }

    let mut output = String::with_capacity(value.len());
    let mut remaining = value;

    while let Some(start) = remaining.find("var(") {
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 4..];
        let Some(close) = find_var_close(after) else {
            output.push_str(remaining);
            return Some(output);
        };
        let body = &after[..close];
        let (name, fallback) = match split_first_comma_outside_parens(body) {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (body.trim(), None),
        };

        if !name.starts_with("--") {
            let fb = fallback?;
            let expanded = resolve_var_recursive(fb, customs, visited, depth + 1)?;
            output.push_str(&expanded);
        } else if visited.contains(name) {
            // Cycle detected!
            let fb = fallback?;
            let expanded = resolve_var_recursive(fb, customs, visited, depth + 1)?;
            output.push_str(&expanded);
        } else if let Some(var_val) = customs.get(name) {
            visited.insert(name.to_string());
            let resolved_candidate = resolve_var_recursive(var_val, customs, visited, depth + 1);
            visited.remove(name);

            if let Some(expanded) = resolved_candidate {
                output.push_str(&expanded);
            } else {
                let fb = fallback?;
                let expanded = resolve_var_recursive(fb, customs, visited, depth + 1)?;
                output.push_str(&expanded);
            }
        } else {
            let fb = fallback?;
            let expanded = resolve_var_recursive(fb, customs, visited, depth + 1)?;
            output.push_str(&expanded);
        }

        remaining = &after[close + 1..];
    }

    output.push_str(remaining);
    Some(output)
}

fn split_first_comma_outside_parens(s: &str) -> Option<(&str, &str)> {
    let mut depth: usize = 0;
    for (idx, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                return Some((&s[..idx], &s[idx + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn find_var_close(after: &str) -> Option<usize> {
    let mut depth: usize = 0;
    for (index, character) in after.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(index);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

/// Nesting budget for calc()/min()/max()/clamp() and parenthesised terms.
/// Hostile stylesheets can otherwise recurse the evaluator to stack overflow.
const MAX_MATH_DEPTH: usize = 64;

pub fn eval_math_expression(
    expr: &str,
    parent_size: f64,
    root_size: f64,
    vw: f64,
    vh: f64,
    customs: &HashMap<String, String>,
) -> Option<f64> {
    eval_math_expression_bounded(expr, parent_size, root_size, vw, vh, customs, 0)
}

fn eval_math_expression_bounded(
    expr: &str,
    parent_size: f64,
    root_size: f64,
    vw: f64,
    vh: f64,
    customs: &HashMap<String, String>,
    depth: usize,
) -> Option<f64> {
    if depth > MAX_MATH_DEPTH {
        return None;
    }
    let resolved = resolve_var_value(expr, customs).unwrap_or_else(|| expr.to_string());
    let trimmed = resolved.trim();

    if let Some(inner) = trimmed
        .strip_prefix("calc(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return eval_calc_terms(inner.trim(), parent_size, root_size, vw, vh, depth + 1);
    }
    if let Some(inner) = trimmed
        .strip_prefix("min(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let args = split_math_args(inner);
        let mut min_val = f64::INFINITY;
        for arg in args {
            if let Some(v) =
                eval_math_expression_bounded(&arg, parent_size, root_size, vw, vh, customs, depth + 1)
            {
                if v < min_val {
                    min_val = v;
                }
            }
        }
        return if min_val.is_finite() {
            Some(min_val)
        } else {
            None
        };
    }
    if let Some(inner) = trimmed
        .strip_prefix("max(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let args = split_math_args(inner);
        let mut max_val = f64::NEG_INFINITY;
        for arg in args {
            if let Some(v) =
                eval_math_expression_bounded(&arg, parent_size, root_size, vw, vh, customs, depth + 1)
            {
                if v > max_val {
                    max_val = v;
                }
            }
        }
        return if max_val.is_finite() {
            Some(max_val)
        } else {
            None
        };
    }
    if let Some(inner) = trimmed
        .strip_prefix("clamp(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let args = split_math_args(inner);
        if args.len() == 3 {
            let min = eval_math_expression_bounded(&args[0], parent_size, root_size, vw, vh, customs, depth + 1)?;
            let val = eval_math_expression_bounded(&args[1], parent_size, root_size, vw, vh, customs, depth + 1)?;
            let max = eval_math_expression_bounded(&args[2], parent_size, root_size, vw, vh, customs, depth + 1)?;
            // CSS clamp(MIN, VAL, MAX) is max(MIN, min(VAL, MAX)): when MIN
            // exceeds MAX the spec resolves to MIN. f64::clamp panics on
            // inverted bounds, so compose min/max instead.
            return Some(val.min(max).max(min));
        }
    }

    CssUnit::parse(trimmed).map(|u| u.to_pixels_with_viewport(parent_size, root_size, vw, vh))
}

fn split_math_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth: usize = 0;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

fn eval_calc_terms(
    inner: &str,
    parent_size: f64,
    root_size: f64,
    vw: f64,
    vh: f64,
    depth: usize,
) -> Option<f64> {
    if depth > MAX_MATH_DEPTH {
        return None;
    }
    let mut total = 0.0;
    let mut current_op = '+';
    let mut current_term = String::new();
    let mut paren_depth: usize = 0;

    for c in inner.chars() {
        match c {
            '(' => {
                paren_depth += 1;
                current_term.push(c);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current_term.push(c);
            }
            '+' | '-' if paren_depth == 0 => {
                let term_val =
                    eval_single_calc_term(current_term.trim(), parent_size, root_size, vw, vh, depth)?;
                if current_op == '+' {
                    total += term_val;
                } else {
                    total -= term_val;
                }
                current_op = c;
                current_term.clear();
            }
            _ => current_term.push(c),
        }
    }

    if !current_term.trim().is_empty() {
        let term_val = eval_single_calc_term(current_term.trim(), parent_size, root_size, vw, vh, depth)?;
        if current_op == '+' {
            total += term_val;
        } else {
            total -= term_val;
        }
    }

    Some(total)
}

fn eval_single_calc_term(
    term: &str,
    parent_size: f64,
    root_size: f64,
    vw: f64,
    vh: f64,
    depth: usize,
) -> Option<f64> {
    if depth > MAX_MATH_DEPTH {
        return None;
    }
    let term = term.trim();
    if term.is_empty() {
        return Some(0.0);
    }
    if let Some((left, right)) = term.split_once('*') {
        let lv = eval_single_calc_term(left.trim(), parent_size, root_size, vw, vh, depth + 1)?;
        let rv = eval_single_calc_term(right.trim(), parent_size, root_size, vw, vh, depth + 1)?;
        return Some(lv * rv);
    }
    if let Some((left, right)) = term.split_once('/') {
        let lv = eval_single_calc_term(left.trim(), parent_size, root_size, vw, vh, depth + 1)?;
        let rv = eval_single_calc_term(right.trim(), parent_size, root_size, vw, vh, depth + 1)?;
        if rv != 0.0 {
            return Some(lv / rv);
        }
        return None;
    }
    if let Some(inner) = term.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return eval_calc_terms(inner, parent_size, root_size, vw, vh, depth + 1);
    }
    CssUnit::parse(term).map(|u| u.to_pixels_with_viewport(parent_size, root_size, vw, vh))
}

pub fn parse_css_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();

    if let Some(hex) = trimmed.strip_prefix('#') {
        match hex.len() {
            3 => {
                let mut chars = hex.chars();
                let r = chars.next()?;
                let g = chars.next()?;
                let b = chars.next()?;
                return Some(format!("#{}{}{}{}{}{}", r, r, g, g, b, b));
            }
            4 => {
                let mut chars = hex.chars();
                let r = chars.next()?;
                let g = chars.next()?;
                let b = chars.next()?;
                let a = chars.next()?;
                return Some(format!("#{}{}{}{}{}{}{}{}", r, r, g, g, b, b, a, a));
            }
            6 | 8 => return Some(trimmed.to_string()),
            _ => return None,
        }
    }

    if lower == "transparent" {
        return Some("transparent".to_string());
    }
    if lower == "currentcolor" {
        return Some("currentColor".to_string());
    }

    if let Some(hex) = lookup_named_color(&lower) {
        return Some(hex.to_string());
    }

    if lower.starts_with("rgb(") || lower.starts_with("rgba(") {
        let inner = trimmed
            .strip_prefix("rgba(")
            .or_else(|| trimmed.strip_prefix("rgb("))?
            .strip_suffix(')')?
            .trim();

        let parts = inner
            .replace('/', ",")
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect::<Vec<_>>();

        if parts.len() >= 3 {
            let r = parse_color_component(&parts[0])?;
            let g = parse_color_component(&parts[1])?;
            let b = parse_color_component(&parts[2])?;
            if parts.len() >= 4 {
                let a = parse_alpha_component(&parts[3])?;
                return Some(format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a));
            }
            return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
        }
    }

    if lower.starts_with("hsl(") || lower.starts_with("hsla(") {
        let inner = trimmed
            .strip_prefix("hsla(")
            .or_else(|| trimmed.strip_prefix("hsl("))?
            .strip_suffix(')')?
            .trim();

        let parts = inner
            .replace('/', ",")
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect::<Vec<_>>();

        if parts.len() >= 3 {
            let h = parse_angle_deg(&parts[0]).unwrap_or(0.0);
            let s = parse_percentage(&parts[1])?;
            let l = parse_percentage(&parts[2])?;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            if parts.len() >= 4 {
                let a = parse_alpha_component(&parts[3])?;
                return Some(format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a));
            }
            return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
        }
    }

    None
}

fn parse_angle_deg(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(deg) = s.strip_suffix("deg") {
        deg.parse::<f64>().ok()
    } else if let Some(rad) = s.strip_suffix("rad") {
        rad.parse::<f64>().ok().map(|r| r.to_degrees())
    } else if let Some(grad) = s.strip_suffix("grad") {
        grad.parse::<f64>().ok().map(|g| g * 360.0 / 400.0)
    } else if let Some(turn) = s.strip_suffix("turn") {
        turn.parse::<f64>().ok().map(|t| t * 360.0)
    } else {
        s.parse::<f64>().ok()
    }
}

fn parse_color_component(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        let p = pct.parse::<f64>().ok()?.clamp(0.0, 100.0);
        Some((p * 255.0 / 100.0).round() as u8)
    } else {
        s.parse::<f64>()
            .ok()
            .map(|v| v.clamp(0.0, 255.0).round() as u8)
    }
}

fn parse_alpha_component(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        let p = pct.parse::<f64>().ok()?.clamp(0.0, 100.0);
        Some((p * 255.0 / 100.0).round() as u8)
    } else {
        s.parse::<f64>()
            .ok()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

fn parse_percentage(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        pct.parse::<f64>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))
    } else {
        s.parse::<f64>().ok().map(|p| p.clamp(0.0, 1.0))
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r_prime, g_prime, b_prime) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r_prime + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g_prime + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b_prime + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn lookup_named_color(name: &str) -> Option<&'static str> {
    match name {
        "black" => Some("#000000"),
        "silver" => Some("#c0c0c0"),
        "gray" | "grey" => Some("#808080"),
        "white" => Some("#ffffff"),
        "maroon" => Some("#800000"),
        "red" => Some("#ff0000"),
        "purple" => Some("#800080"),
        "fuchsia" | "magenta" => Some("#ff00ff"),
        "green" => Some("#008000"),
        "lime" => Some("#00ff00"),
        "olive" => Some("#808000"),
        "yellow" => Some("#ffff00"),
        "navy" => Some("#000080"),
        "blue" => Some("#0000ff"),
        "teal" => Some("#008080"),
        "aqua" | "cyan" => Some("#00ffff"),
        "orange" => Some("#ffa500"),
        "aliceblue" => Some("#f0f8ff"),
        "antiquewhite" => Some("#faebd7"),
        "aquamarine" => Some("#7fffd4"),
        "azure" => Some("#f0ffff"),
        "beige" => Some("#f5f5dc"),
        "bisque" => Some("#ffe4c4"),
        "blanchedalmond" => Some("#ffebcd"),
        "blueviolet" => Some("#8a2be2"),
        "brown" => Some("#a52a2a"),
        "burlywood" => Some("#deb887"),
        "cadetblue" => Some("#5f9ea0"),
        "chartreuse" => Some("#7fff00"),
        "chocolate" => Some("#d2691e"),
        "coral" => Some("#ff7f50"),
        "cornflowerblue" => Some("#6495ed"),
        "cornsilk" => Some("#fff8dc"),
        "crimson" => Some("#dc143c"),
        "darkblue" => Some("#00008b"),
        "darkcyan" => Some("#008b8b"),
        "darkgoldenrod" => Some("#b8860b"),
        "darkgray" | "darkgrey" => Some("#a9a9a9"),
        "darkgreen" => Some("#006400"),
        "darkkhaki" => Some("#bdb76b"),
        "darkmagenta" => Some("#8b008b"),
        "darkolivegreen" => Some("#556b2f"),
        "darkorange" => Some("#ff8c00"),
        "darkorchid" => Some("#9932cc"),
        "darkred" => Some("#8b0000"),
        "darksalmon" => Some("#e9967a"),
        "darkseagreen" => Some("#8fbc8f"),
        "darkslateblue" => Some("#483d8b"),
        "darkslategray" | "darkslategrey" => Some("#2f4f4f"),
        "darkturquoise" => Some("#00ced1"),
        "darkviolet" => Some("#9400d3"),
        "deeppink" => Some("#ff1493"),
        "deepskyblue" => Some("#00bfff"),
        "dimgray" | "dimgrey" => Some("#696969"),
        "dodgerblue" => Some("#1e90ff"),
        "firebrick" => Some("#b22222"),
        "floralwhite" => Some("#fffaf0"),
        "forestgreen" => Some("#228b22"),
        "gainsboro" => Some("#dcdcdc"),
        "ghostwhite" => Some("#f8f8ff"),
        "gold" => Some("#ffd700"),
        "goldenrod" => Some("#daa520"),
        "greenyellow" => Some("#adff2f"),
        "honeydew" => Some("#f0fff0"),
        "hotpink" => Some("#ff69b4"),
        "indianred" => Some("#cd5c5c"),
        "indigo" => Some("#4b0082"),
        "ivory" => Some("#fffff0"),
        "khaki" => Some("#f0e68c"),
        "lavender" => Some("#e6e6fa"),
        "lavenderblush" => Some("#fff0f5"),
        "lawngreen" => Some("#7cfc00"),
        "lemonchiffon" => Some("#fffacd"),
        "lightblue" => Some("#add8e6"),
        "lightcoral" => Some("#f08080"),
        "lightcyan" => Some("#e0ffff"),
        "lightgoldenrodyellow" => Some("#fafad2"),
        "lightgray" | "lightgrey" => Some("#d3d3d3"),
        "lightgreen" => Some("#90ee90"),
        "lightpink" => Some("#ffb6c1"),
        "lightsalmon" => Some("#ffa07a"),
        "lightseagreen" => Some("#20b2aa"),
        "lightskyblue" => Some("#87cefa"),
        "lightslategray" | "lightslategrey" => Some("#778899"),
        "lightsteelblue" => Some("#b0c4de"),
        "lightyellow" => Some("#ffffe0"),
        "limegreen" => Some("#32cd32"),
        "linen" => Some("#faf0e6"),
        "mediumaquamarine" => Some("#66cdaa"),
        "mediumblue" => Some("#0000cd"),
        "mediumorchid" => Some("#ba55d3"),
        "mediumpurple" => Some("#9370db"),
        "mediumseagreen" => Some("#3cb371"),
        "mediumslateblue" => Some("#7b68ee"),
        "mediumspringgreen" => Some("#00fa9a"),
        "mediumturquoise" => Some("#48d1cc"),
        "mediumvioletred" => Some("#c71585"),
        "midnightblue" => Some("#191970"),
        "mintcream" => Some("#f5fffa"),
        "mistyrose" => Some("#ffe4e1"),
        "moccasin" => Some("#ffe4b5"),
        "navajowhite" => Some("#ffdead"),
        "oldlace" => Some("#fdf5e6"),
        "olivedrab" => Some("#6b8e23"),
        "orangered" => Some("#ff4500"),
        "orchid" => Some("#da70d6"),
        "palegoldenrod" => Some("#eee8aa"),
        "palegreen" => Some("#98fb98"),
        "paleturquoise" => Some("#afeeee"),
        "palevioletred" => Some("#db7093"),
        "papayawhip" => Some("#ffefd5"),
        "peachpuff" => Some("#ffdab9"),
        "peru" => Some("#cd853f"),
        "pink" => Some("#ffc0cb"),
        "plum" => Some("#dda0dd"),
        "powderblue" => Some("#b0e0e6"),
        "rebeccapurple" => Some("#663399"),
        "rosybrown" => Some("#bc8f8f"),
        "royalblue" => Some("#4169e1"),
        "saddlebrown" => Some("#8b4513"),
        "salmon" => Some("#fa8072"),
        "sandybrown" => Some("#f4a460"),
        "seagreen" => Some("#2e8b57"),
        "seashell" => Some("#fff5ee"),
        "sienna" => Some("#a0522d"),
        "skyblue" => Some("#87ceeb"),
        "slateblue" => Some("#6a5acd"),
        "slategray" | "slategrey" => Some("#708090"),
        "snow" => Some("#fffafa"),
        "springgreen" => Some("#00ff7f"),
        "steelblue" => Some("#4682b4"),
        "tan" => Some("#d2b48c"),
        "thistle" => Some("#d8bfd8"),
        "tomato" => Some("#ff6347"),
        "turquoise" => Some("#40e0d0"),
        "violet" => Some("#ee82ee"),
        "wheat" => Some("#f5deb3"),
        "whitesmoke" => Some("#f5f5f5"),
        "yellowgreen" => Some("#9acd32"),
        _ => None,
    }
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let chars: Vec<char> = css.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = None;

    while i < len {
        let c = chars[i];
        if let Some(quote) = in_string {
            out.push(c);
            if c == '\\' && i + 1 < len {
                i += 1;
                out.push(chars[i]);
            } else if c == quote {
                in_string = None;
            }
            i += 1;
        } else if c == '"' || c == '\'' {
            in_string = Some(c);
            out.push(c);
            i += 1;
        } else if c == '/' && i + 1 < len && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

pub fn parse_css(css: &str) -> Vec<CssRule> {
    parse_css_with_media(css, 0)
}

pub fn parse_css_with_media(css: &str, viewport_width: u32) -> Vec<CssRule> {
    let mut layer_map = HashMap::new();
    parse_css_with_context(
        css,
        viewport_width,
        CssOrigin::Author,
        None,
        None,
        0,
        &mut HashSet::new(),
        &mut layer_map,
    )
}

#[allow(clippy::too_many_arguments)]
fn parse_css_with_context(
    css: &str,
    viewport_width: u32,
    origin: CssOrigin,
    layer: Option<String>,
    layer_order: Option<usize>,
    recursion_depth: usize,
    visited_imports: &mut HashSet<String>,
    layer_map: &mut HashMap<String, usize>,
) -> Vec<CssRule> {
    if recursion_depth > 16 || css.len() > 10_000_000 {
        return Vec::new();
    }

    let mut rules = Vec::new();
    let stripped = strip_css_comments(css);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return rules;
    }

    let chars: Vec<char> = stripped.chars().collect();
    let len = chars.len();
    let mut pos = 0;
    let mut source_order = 0;

    while pos < len {
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        if pos >= len {
            break;
        }

        if chars[pos] == '@' {
            let at_start = pos;
            while pos < len && chars[pos] != ';' && chars[pos] != '{' {
                pos += 1;
            }
            if pos >= len {
                break;
            }

            let at_header: String = chars[at_start..pos].iter().collect();
            let at_trimmed = at_header.trim();

            if chars[pos] == ';' {
                pos += 1;
                if let Some(import_stmt) = at_trimmed.strip_prefix("@import") {
                    let import_stmt = import_stmt.trim();
                    if let Some(url) = parse_import_url(import_stmt) {
                        if !visited_imports.contains(&url) && visited_imports.len() < 32 {
                            visited_imports.insert(url.clone());
                        }
                    }
                } else if let Some(layer_names) = at_trimmed.strip_prefix("@layer") {
                    let layer_names = layer_names.trim();
                    for name in layer_names.split(',') {
                        let name = name.trim().to_string();
                        if !name.is_empty() && !layer_map.contains_key(&name) {
                            let next_idx = layer_map.len();
                            layer_map.insert(name, next_idx);
                        }
                    }
                }
                continue;
            }

            pos += 1;
            let body_start = pos;
            let mut depth: usize = 1;
            let mut in_str = None;

            while pos < len && depth > 0 {
                let c = chars[pos];
                if let Some(q) = in_str {
                    if c == '\\' && pos + 1 < len {
                        pos += 1;
                    } else if c == q {
                        in_str = None;
                    }
                } else if c == '"' || c == '\'' {
                    in_str = Some(c);
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth = depth.saturating_sub(1);
                }
                pos += 1;
            }

            let body: String = chars[body_start..pos.saturating_sub(1)].iter().collect();

            if let Some(condition) = at_trimmed.strip_prefix("@media") {
                let condition = condition.trim();
                if viewport_width > 0 && media_query_matches(condition, viewport_width) {
                    rules.extend(parse_css_with_context(
                        &body,
                        viewport_width,
                        origin,
                        layer.clone(),
                        layer_order,
                        recursion_depth + 1,
                        visited_imports,
                        layer_map,
                    ));
                }
            } else if let Some(condition) = at_trimmed.strip_prefix("@supports") {
                let condition = condition.trim();
                if supports_query_matches(condition) {
                    rules.extend(parse_css_with_context(
                        &body,
                        viewport_width,
                        origin,
                        layer.clone(),
                        layer_order,
                        recursion_depth + 1,
                        visited_imports,
                        layer_map,
                    ));
                }
            } else if let Some(layer_name) = at_trimmed.strip_prefix("@layer") {
                let layer_name = layer_name.trim();
                let effective_layer = if layer_name.is_empty() {
                    Some("anonymous".to_string())
                } else {
                    Some(layer_name.to_string())
                };

                let effective_order = if let Some(ref lname) = effective_layer {
                    let next_idx = layer_map.len();
                    Some(*layer_map.entry(lname.clone()).or_insert(next_idx))
                } else {
                    None
                };

                rules.extend(parse_css_with_context(
                    &body,
                    viewport_width,
                    origin,
                    effective_layer,
                    effective_order,
                    recursion_depth + 1,
                    visited_imports,
                    layer_map,
                ));
            }

            continue;
        }

        let sel_start = pos;
        let mut brace_depth: usize = 0;
        let mut in_str = None;

        while pos < len && !(chars[pos] == '{' && brace_depth == 0 && in_str.is_none()) {
            let c = chars[pos];
            if let Some(q) = in_str {
                if c == '\\' && pos + 1 < len {
                    pos += 1;
                } else if c == q {
                    in_str = None;
                }
            } else if c == '"' || c == '\'' {
                in_str = Some(c);
            } else if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth = brace_depth.saturating_sub(1);
            }
            pos += 1;
        }

        if pos >= len {
            break;
        }

        let selector_str: String = chars[sel_start..pos].iter().collect();
        pos += 1;

        let mut declarations = Vec::new();
        while pos < len && chars[pos] != '}' {
            while pos < len && chars[pos].is_whitespace() {
                pos += 1;
            }
            if pos >= len || chars[pos] == '}' {
                break;
            }

            let prop_start = pos;
            while pos < len && chars[pos] != ':' && chars[pos] != '}' && chars[pos] != ';' {
                pos += 1;
            }
            if pos >= len || chars[pos] == '}' || chars[pos] == ';' {
                if pos < len && chars[pos] == ';' {
                    pos += 1;
                }
                continue;
            }

            let property: String = chars[prop_start..pos].iter().collect();
            let property = property.trim().to_ascii_lowercase();
            pos += 1;

            let val_start = pos;
            let mut val_paren_depth: usize = 0;
            let mut val_in_str = None;

            while pos < len
                && !(chars[pos] == ';' && val_paren_depth == 0 && val_in_str.is_none())
                && !(chars[pos] == '}' && val_in_str.is_none())
            {
                let c = chars[pos];
                if let Some(q) = val_in_str {
                    if c == '\\' && pos + 1 < len {
                        pos += 1;
                    } else if c == q {
                        val_in_str = None;
                    }
                } else if c == '"' || c == '\'' {
                    val_in_str = Some(c);
                } else if c == '(' {
                    val_paren_depth += 1;
                } else if c == ')' {
                    val_paren_depth = val_paren_depth.saturating_sub(1);
                }
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
                source_order += 1;
                declarations.push(Declaration {
                    property,
                    value,
                    important,
                    origin,
                    layer: layer.clone(),
                    layer_order,
                    specificity: (0, 0, 0),
                    source_order,
                });
            }
        }

        if pos < len && chars[pos] == '}' {
            pos += 1;
        }

        let selector_str = selector_str.trim();
        if !selector_str.is_empty() {
            let selectors = parse_selector_list(selector_str);
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
                    origin,
                    layer: layer.clone(),
                    layer_order,
                    source_order,
                });
            }
        }
    }

    rules
}

fn parse_import_url(stmt: &str) -> Option<String> {
    let stmt = stmt.trim();
    if let Some(inner) = stmt.strip_prefix("url(").and_then(|s| s.split(')').next()) {
        return Some(
            inner
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string(),
        );
    }
    if stmt.starts_with('"') || stmt.starts_with('\'') {
        let quote = stmt.chars().next()?;
        let end = stmt[1..].find(quote)?;
        return Some(stmt[1..=end].to_string());
    }
    None
}

/// Split at top-level occurrences of a keyword such as " and " / " or ",
/// ignoring matches inside parentheses.
fn split_supports_top_level<'a>(input: &'a str, keyword: &str) -> Vec<&'a str> {
    let lowered = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let lower_bytes = lowered.as_bytes();
    let kw = keyword.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                paren_depth += 1;
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            }
            b' ' if paren_depth == 0 && lower_bytes[i..].starts_with(kw) => {
                parts.push(&input[start..i]);
                i += kw.len();
                start = i;
            }
            _ => i += 1,
        }
    }
    parts.push(&input[start..]);
    parts
}

pub fn supports_query_matches(condition: &str) -> bool {
    supports_query_matches_bounded(condition.trim(), 0)
}

fn supports_query_matches_bounded(condition: &str, depth: usize) -> bool {
    if depth > MAX_MATH_DEPTH {
        return false;
    }
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Count leading `not` prefixes iteratively so a hostile condition
    // cannot recurse once per token to stack overflow.
    let mut negations = 0usize;
    let mut rest = trimmed;
    loop {
        let has_not = matches!(rest.get(..4), Some(prefix) if prefix.eq_ignore_ascii_case("not "));
        if !has_not {
            break;
        }
        negations += 1;
        rest = rest[4..].trim_start();
    }
    if negations > 0 && !rest.is_empty() {
        let inner = supports_query_matches_bounded(rest, depth + 1);
        return if negations % 2 == 1 { !inner } else { inner };
    }
    // CSS precedence: `or` binds looser than `and`, so split on `or`
    // first and evaluate each disjunct as an `and` conjunction.
    if trimmed.contains(" or ") || trimmed.contains(" OR ") {
        return split_supports_top_level(trimmed, " or ")
            .into_iter()
            .any(|clause| supports_query_matches_bounded(clause, depth + 1));
    }
    if trimmed.contains(" and ") || trimmed.contains(" AND ") {
        return split_supports_top_level(trimmed, " and ")
            .into_iter()
            .all(|clause| supports_query_matches_bounded(clause, depth + 1));
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if let Some((prop, val)) = inner.split_once(':') {
            return is_property_value_supported(prop.trim(), val.trim());
        }
        if let Some(sel) = inner.strip_prefix("selector(") {
            if let Some(sel) = sel.strip_suffix(')') {
                return !parse_selector_list(sel).is_empty();
            }
        }
    }
    true
}

fn is_property_value_supported(prop: &str, _val: &str) -> bool {
    matches!(
        prop.to_ascii_lowercase().as_str(),
        "display"
            | "position"
            | "color"
            | "background"
            | "background-color"
            | "width"
            | "height"
            | "min-width"
            | "max-width"
            | "margin"
            | "padding"
            | "border"
            | "flex"
            | "flex-direction"
            | "grid"
            | "grid-template-columns"
            | "gap"
            | "transform"
            | "transition"
            | "animation"
            | "opacity"
            | "overflow"
            | "z-index"
    )
}

pub fn compute_computed_style(
    element_tag: &str,
    element_classes: &[String],
    element_id: Option<&str>,
    rules: &[CssRule],
    parent_style: Option<&ComputedStyle>,
    element_attrs: &HashMap<String, String>,
) -> ComputedStyle {
    let is_root =
        element_tag.eq_ignore_ascii_case("html") || element_tag.eq_ignore_ascii_case(":root");
    compute_computed_style_with_ancestors(
        element_tag,
        element_classes,
        element_id,
        rules,
        parent_style,
        element_attrs,
        is_root,
        &[],
    )
}

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
    compute_computed_style_full(
        element_tag,
        element_classes,
        element_id,
        rules,
        parent_style,
        element_attrs,
        is_root,
        ancestry,
        &SiblingContext::default(),
    )
}

/// Full entry point: like [`compute_computed_style_with_ancestors`] but with
/// real sibling-position facts so structural pseudo-classes and `+`/`~`
/// combinators evaluate against the actual document instead of defaults.
#[allow(clippy::too_many_arguments)]
pub fn compute_computed_style_full(
    element_tag: &str,
    element_classes: &[String],
    element_id: Option<&str>,
    rules: &[CssRule],
    parent_style: Option<&ComputedStyle>,
    element_attrs: &HashMap<String, String>,
    is_root: bool,
    ancestry: &[ElementAncestry],
    siblings: &SiblingContext,
) -> ComputedStyle {
    let is_root_elem = is_root
        || element_tag.eq_ignore_ascii_case("html")
        || element_tag.eq_ignore_ascii_case(":root");

    let matching_ctx = ElementMatchingContext::with_siblings(
        element_tag,
        element_classes,
        element_id,
        element_attrs,
        is_root_elem,
        ancestry,
        siblings,
    );

    let mut style = if let Some(parent) = parent_style {
        ComputedStyle {
            color: parent.color.clone(),
            font_family: parent.font_family.clone(),
            font_size: parent.font_size.clone(),
            font_weight: parent.font_weight,
            font_style: parent.font_style.clone(),
            text_align: parent.text_align.clone(),
            line_height: parent.line_height,
            letter_spacing: parent.letter_spacing.clone(),
            word_spacing: parent.word_spacing.clone(),
            white_space: parent.white_space.clone(),
            visibility: parent.visibility.clone(),
            cursor: parent.cursor.clone(),
            custom_properties: parent.custom_properties.clone(),
            ..ComputedStyle::default()
        }
    } else {
        ComputedStyle::default()
    };

    style.display = Some(default_display_for_tag(element_tag).to_string());
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

    struct MatchedDecl<'a> {
        decl: &'a Declaration,
        origin: CssOrigin,
        layer_order: Option<usize>,
        specificity: (u32, u32, u32),
        source_order: usize,
        is_inline: bool,
    }

    let mut matched_declarations: Vec<MatchedDecl> = Vec::new();

    for (rule_idx, rule) in rules.iter().enumerate() {
        // A grouped rule matches once; the cascade uses the HIGHEST
        // specificity among its matching selectors, not the first match.
        let mut best_specificity: Option<(u32, u32, u32)> = None;
        for selector in &rule.selectors {
            if selector.matches_context(&matching_ctx) {
                let spec = selector.specificity();
                if best_specificity.is_none_or(|best| spec > best) {
                    best_specificity = Some(spec);
                }
            }
        }
        if let Some(spec) = best_specificity {
            for decl in &rule.declarations {
                matched_declarations.push(MatchedDecl {
                    decl,
                    origin: rule.origin,
                    layer_order: rule.layer_order,
                    specificity: spec,
                    source_order: rule_idx * 1000 + decl.source_order,
                    is_inline: false,
                });
            }
        }
    }

    let inline_declarations = element_attrs
        .get("style")
        .map(|inline| parse_inline_declarations(inline))
        .unwrap_or_default();

    for (idx, decl) in inline_declarations.iter().enumerate() {
        matched_declarations.push(MatchedDecl {
            decl,
            origin: CssOrigin::Author,
            layer_order: None,
            specificity: (1, 0, 0),
            source_order: usize::MAX - 1000 + idx,
            is_inline: true,
        });
    }

    matched_declarations.sort_by(|a, b| {
        let rank_a = cascade_rank(
            a.origin,
            a.decl.important,
            a.is_inline,
            a.layer_order,
            a.specificity,
            a.source_order,
        );
        let rank_b = cascade_rank(
            b.origin,
            b.decl.important,
            b.is_inline,
            b.layer_order,
            b.specificity,
            b.source_order,
        );
        rank_a.cmp(&rank_b)
    });

    for item in &matched_declarations {
        if item.decl.property.trim().starts_with("--") {
            style.apply_declaration_resolved(item.decl, &style.custom_properties.clone());
        }
    }

    for item in &matched_declarations {
        if !item.decl.property.trim().starts_with("--") {
            style.apply_declaration_resolved(item.decl, &style.custom_properties.clone());
        }
    }

    style
}

fn cascade_rank(
    origin: CssOrigin,
    important: bool,
    is_inline: bool,
    layer_order: Option<usize>,
    specificity: (u32, u32, u32),
    source_order: usize,
) -> (u8, u32, (u32, u32, u32), usize) {
    let group = match (origin, important, is_inline) {
        (CssOrigin::UserAgent, true, _) => 8,
        (CssOrigin::User, true, _) => 7,
        (CssOrigin::Author, true, true) => 6,
        (CssOrigin::Author, true, false) => 5,
        (CssOrigin::Author, false, true) => 4,
        (CssOrigin::Author, false, false) => 3,
        (CssOrigin::User, false, _) => 2,
        (CssOrigin::UserAgent, false, _) => 1,
    };

    let layer_rank = if important {
        match layer_order {
            None => 0,
            Some(idx) => u32::MAX - 1 - (idx as u32),
        }
    } else {
        match layer_order {
            None => u32::MAX,
            Some(idx) => idx as u32,
        }
    };

    (group, layer_rank, specificity, source_order)
}

fn parse_inline_declarations(s: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((prop, value)) = part.split_once(':') {
            let prop = prop.trim().to_ascii_lowercase();
            let (value, important) = strip_important(value.trim());
            let value = value.to_string();
            if !prop.is_empty() && !value.is_empty() {
                out.push(Declaration {
                    property: prop,
                    value,
                    important,
                    origin: CssOrigin::Author,
                    layer: None,
                    layer_order: None,
                    specificity: (0, 0, 0),
                    source_order: 0,
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

pub fn parse_class_attr(class_attr: Option<&str>) -> Vec<String> {
    class_attr
        .map(|s| s.split_whitespace().map(|c| c.to_string()).collect())
        .unwrap_or_default()
}

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
        if let Some((left, right)) = inner.split_once("<=") {
            if let Some(val) = parse_media_length(right) {
                if left.trim() == "width" {
                    return viewport_width <= val;
                }
            }
        }
        if let Some((left, right)) = inner.split_once(">=") {
            if let Some(val) = parse_media_length(right) {
                if left.trim() == "width" {
                    return viewport_width >= val;
                }
            }
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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CssDiagnostics {
    pub unsupported_selectors: HashMap<String, usize>,
    pub unsupported_at_rules: HashMap<String, usize>,
    pub unsupported_properties: HashMap<String, usize>,
    pub unsupported_values: HashMap<String, usize>,
    pub parse_errors: Vec<CssDiagnosticRecord>,
    pub total_rules_parsed: usize,
    pub total_bytes_parsed: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CssDiagnosticRecord {
    pub error_type: String,
    pub snippet: String,
    pub reason: String,
}

static DIAGNOSTICS: OnceLock<Mutex<CssDiagnostics>> = OnceLock::new();

fn diagnostics_lock() -> &'static Mutex<CssDiagnostics> {
    DIAGNOSTICS.get_or_init(|| Mutex::new(CssDiagnostics::default()))
}

pub fn record_unsupported_property(property: &str) {
    if let Ok(mut diag) = diagnostics_lock().lock() {
        let count = diag
            .unsupported_properties
            .entry(property.to_string())
            .or_insert(0);
        *count = count.saturating_add(1);
    }
}

pub fn get_css_diagnostics() -> CssDiagnostics {
    diagnostics_lock()
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default()
}

pub fn reset_css_diagnostics() {
    if let Ok(mut diag) = diagnostics_lock().lock() {
        *diag = CssDiagnostics::default();
    }
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
        assert_eq!(CssUnit::Auto.to_pixels(800.0, 16.0), 0.0);
    }

    #[test]
    fn test_stray_brace_does_not_hang() {
        let css = "div } { color: red; }";
        let rules = parse_css(css);
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
        let rules = parse_css("p, { color: red; }");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors.len(), 1);
        assert_eq!(rules[0].selectors[0].tag.as_deref(), Some("p"));
    }

    #[test]
    fn test_comment_inside_selector_does_not_kill_rule() {
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
        let css = "@media screen { body { color: red; } } p { color: blue; }";
        let rules = parse_css(css);
        assert_eq!(rules.len(), 1, "rules: {:?}", rules);
        assert_eq!(rules[0].selectors[0].tag.as_deref(), Some("p"));
    }

    #[test]
    fn test_grouped_selector_specificity_is_per_selector() {
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

    #[test]
    fn test_calc_and_math_functions() {
        let val = eval_math_expression(
            "calc(100px - 20px)",
            500.0,
            16.0,
            1000.0,
            800.0,
            &HashMap::new(),
        );
        assert_eq!(val, Some(80.0));

        let val_clamp = eval_math_expression(
            "clamp(10px, 50px, 100px)",
            500.0,
            16.0,
            1000.0,
            800.0,
            &HashMap::new(),
        );
        assert_eq!(val_clamp, Some(50.0));

        let val_min = eval_math_expression(
            "min(20px, 40px)",
            500.0,
            16.0,
            1000.0,
            800.0,
            &HashMap::new(),
        );
        assert_eq!(val_min, Some(20.0));

        let val_max = eval_math_expression(
            "max(20px, 40px)",
            500.0,
            16.0,
            1000.0,
            800.0,
            &HashMap::new(),
        );
        assert_eq!(val_max, Some(40.0));
    }

    #[test]
    fn test_color_parsing_advanced() {
        assert_eq!(parse_css_color("#ff0000"), Some("#ff0000".to_string()));
        assert_eq!(parse_css_color("#f00"), Some("#ff0000".to_string()));
        assert_eq!(
            parse_css_color("rgb(255, 0, 0)"),
            Some("#ff0000".to_string())
        );
        assert_eq!(parse_css_color("rgb(255 0 0)"), Some("#ff0000".to_string()));
        assert_eq!(
            parse_css_color("hsl(0, 100%, 50%)"),
            Some("#ff0000".to_string())
        );
        assert_eq!(
            parse_css_color("rebeccapurple"),
            Some("#663399".to_string())
        );
        assert_eq!(
            parse_css_color("transparent"),
            Some("transparent".to_string())
        );
    }

    #[test]
    fn test_pseudo_classes_and_attributes() {
        let sel = Selector::parse("button.btn[type^='sub']:hover:not([disabled])");
        let mut attrs = HashMap::new();
        attrs.insert("type".to_string(), "submit".to_string());
        let classes = vec!["btn".to_string()];
        let ctx = ElementMatchingContext {
            tag: "button",
            classes: &classes,
            id: None,
            attrs: &attrs,
            is_root: false,
            ancestors: &[],
            index_in_parent: 1,
            siblings_after: 0,
            total_siblings: 1,
            type_index_in_parent: 1,
            total_type_siblings: 1,
            is_hovered: true,
            is_focused: false,
            is_active: false,
            is_checked: false,
            is_disabled: false,
            is_empty: false,
            is_target: false,
            is_visited: false,
            previous_siblings: &[],
        };
        assert!(sel.matches_context(&ctx));

        let mut disabled_attrs = HashMap::new();
        disabled_attrs.insert("type".to_string(), "submit".to_string());
        disabled_attrs.insert("disabled".to_string(), "".to_string());
        let disabled_ctx = ElementMatchingContext {
            tag: "button",
            classes: &classes,
            id: None,
            attrs: &disabled_attrs,
            is_root: false,
            ancestors: &[],
            index_in_parent: 1,
            siblings_after: 0,
            total_siblings: 1,
            type_index_in_parent: 1,
            total_type_siblings: 1,
            is_hovered: true,
            is_focused: false,
            is_active: false,
            is_checked: false,
            is_disabled: true,
            is_empty: false,
            is_target: false,
            is_visited: false,
            previous_siblings: &[],
        };
        assert!(!sel.matches_context(&disabled_ctx));
    }
}
