//! Cosmetic Content Control & Element Hiding Rules for GhitaBrowser (Phase 25).
//! Implements domain-specific element hiding rules and cosmetic CSS generation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosmeticFilterRule {
    pub selector: String,
    pub domain: Option<String>,
}

pub struct ContentControlEngine {
    pub rules: Vec<CosmeticFilterRule>,
}

impl ContentControlEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn add_rule(&mut self, selector: impl Into<String>, domain: Option<String>) {
        let selector = selector.into();
        if self.rules.len() >= 10_000
            || selector.is_empty()
            || selector.len() > 4096
            || selector.contains(['{', '}', ';'])
            || domain.as_ref().is_some_and(|domain| {
                domain.len() > 253
                    || domain.is_empty()
                    || !domain.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
                    })
            })
        {
            return;
        }
        self.rules.push(CosmeticFilterRule {
            selector,
            domain: domain.map(|domain| domain.to_ascii_lowercase()),
        });
    }

    pub fn generate_cosmetic_css_for_origin(&self, origin: &str) -> String {
        let domain = url::Url::parse(origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();

        let mut matching_selectors = Vec::new();
        for rule in &self.rules {
            match &rule.domain {
                Some(rule_dom) => {
                    if domain_matches(&domain, rule_dom) {
                        matching_selectors.push(rule.selector.as_str());
                    }
                }
                None => {
                    matching_selectors.push(rule.selector.as_str());
                }
            }
        }

        if matching_selectors.is_empty() {
            String::new()
        } else {
            format!(
                "{} {{ display: none !important; visibility: hidden !important; }}",
                matching_selectors.join(", ")
            )
        }
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

impl Default for ContentControlEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosmetic_filtering_css_generation() {
        let mut engine = ContentControlEngine::new();
        engine.add_rule(".ad-banner", None); // Global rule
        engine.add_rule("#sponsor-box", Some("example.com".to_string())); // Site-specific rule
        engine.add_rule(".other-site-ad", Some("other.com".to_string()));

        let css = engine.generate_cosmetic_css_for_origin("https://example.com/page");
        assert!(css.contains(".ad-banner"));
        assert!(css.contains("#sponsor-box"));
        assert!(!css.contains(".other-site-ad"));
        assert!(css.contains("display: none !important;"));
    }
}
