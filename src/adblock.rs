//! GhitaBrowser's clean-room request and cosmetic filtering engine.
//!
//! The rule grammar is intentionally small and project-specific:
//! `BLOCK host-label=ads types=script,image third-party=true`
//! `ALLOW host=static.example.test page=example.test`

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const MAX_CUSTOM_RULES: usize = 4_096;
const MAX_RULE_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceType {
    Document,
    Script,
    Style,
    Image,
    Font,
    Media,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdBlockConfig {
    pub enabled: bool,
    pub block_trackers: bool,
    pub cosmetic_filtering: bool,
    pub disabled_domains: HashSet<String>,
    pub custom_rules: Vec<String>,
}

impl Default for AdBlockConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_trackers: true,
            cosmetic_filtering: true,
            disabled_domains: HashSet::new(),
            custom_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdBlockStats {
    pub evaluated_count: usize,
    pub blocked_ads_count: usize,
    pub blocked_trackers_count: usize,
    pub allowed_by_exception_count: usize,
    pub blocked_by_resource: BTreeMap<ResourceType, usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleAction {
    Allow,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleMatcher {
    HostSuffix(String),
    HostLabel(String),
    PathSegment(String),
    FileName(String),
    QueryKey(String),
}

#[derive(Debug, Clone)]
struct NetworkRule {
    _id: String,
    action: RuleAction,
    matcher: RuleMatcher,
    resource_types: HashSet<ResourceType>,
    third_party: Option<bool>,
    page_domains: Vec<String>,
    tracker: bool,
}

pub struct AdBlocker {
    config: AdBlockConfig,
    stats: AdBlockStats,
    rules: Vec<NetworkRule>,
    rejected_rule_count: usize,
}

impl AdBlocker {
    pub fn new(mut config: AdBlockConfig) -> Self {
        config.disabled_domains = config
            .disabled_domains
            .into_iter()
            .map(|domain| normalize_host(&domain))
            .filter(|domain| !domain.is_empty())
            .collect();
        config.custom_rules.truncate(MAX_CUSTOM_RULES);

        let mut rules = built_in_rules();
        let mut rejected_rule_count = 0;
        for (index, source) in config.custom_rules.iter().enumerate() {
            match parse_rule(source, format!("custom-{index}")) {
                Ok(rule) => rules.push(rule),
                Err(_) => rejected_rule_count += 1,
            }
        }

        Self {
            config,
            stats: AdBlockStats::default(),
            rules,
            rejected_rule_count,
        }
    }

    /// Backwards-compatible entry point for callers that do not know the
    /// resource type yet.
    pub fn should_block(&mut self, url: &str, page_domain: Option<&str>) -> bool {
        self.should_block_resource(url, page_domain, ResourceType::Other)
    }

    pub fn should_block_resource(
        &mut self,
        url: &str,
        page_url_or_domain: Option<&str>,
        resource_type: ResourceType,
    ) -> bool {
        if !self.config.enabled {
            return false;
        }

        let page_host = page_url_or_domain.map(normalize_host).unwrap_or_default();
        if !page_host.is_empty() && self.is_disabled_for_host(&page_host) {
            return false;
        }

        let Some(request) = RequestParts::parse(url) else {
            return false;
        };
        self.stats.evaluated_count = self.stats.evaluated_count.saturating_add(1);
        let third_party = !page_host.is_empty() && !same_site(&request.host, &page_host);

        // Explicit ALLOW rules have priority so users can recover a site from
        // a false positive without disabling protection for the whole site.
        if self.rules.iter().any(|rule| {
            rule.action == RuleAction::Allow
                && rule_matches(rule, &request, &page_host, resource_type, third_party)
        }) {
            self.stats.allowed_by_exception_count =
                self.stats.allowed_by_exception_count.saturating_add(1);
            return false;
        }

        let matched = self.rules.iter().find(|rule| {
            rule.action == RuleAction::Block
                && (self.config.block_trackers || !rule.tracker)
                && rule_matches(rule, &request, &page_host, resource_type, third_party)
        });
        let Some(rule) = matched else {
            return false;
        };

        if rule.tracker {
            self.stats.blocked_trackers_count = self.stats.blocked_trackers_count.saturating_add(1);
        } else {
            self.stats.blocked_ads_count = self.stats.blocked_ads_count.saturating_add(1);
        }
        *self
            .stats
            .blocked_by_resource
            .entry(resource_type)
            .or_default() += 1;
        true
    }

    pub fn cosmetic_selectors(&self, page_url_or_domain: &str) -> Vec<&'static str> {
        let host = normalize_host(page_url_or_domain);
        if !self.config.enabled
            || !self.config.cosmetic_filtering
            || self.is_disabled_for_host(&host)
        {
            return Vec::new();
        }
        vec![
            "[data-ghita-ad]",
            "[aria-label='Advertisement']",
            ".sponsored-content",
        ]
    }

    pub fn stats(&self) -> &AdBlockStats {
        &self.stats
    }

    pub fn total_blocked(&self) -> usize {
        self.stats.blocked_ads_count + self.stats.blocked_trackers_count
    }

    pub fn toggle_domain(&mut self, domain: String) -> bool {
        let domain = normalize_host(&domain);
        if domain.is_empty() {
            return true;
        }
        if self.config.disabled_domains.remove(&domain) {
            true
        } else {
            self.config.disabled_domains.insert(domain);
            false
        }
    }

    pub fn is_domain_enabled(&self, domain: &str) -> bool {
        !self.is_disabled_for_host(&normalize_host(domain))
    }

    pub fn config(&self) -> &AdBlockConfig {
        &self.config
    }

    pub fn rejected_rule_count(&self) -> usize {
        self.rejected_rule_count
    }

    fn is_disabled_for_host(&self, host: &str) -> bool {
        self.config
            .disabled_domains
            .iter()
            .any(|domain| host_matches(host, domain))
    }
}

#[derive(Debug)]
struct RequestParts {
    host: String,
    path_segments: Vec<String>,
    file_name: String,
    query_keys: HashSet<String>,
}

impl RequestParts {
    fn parse(source: &str) -> Option<Self> {
        let url = url::Url::parse(source).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = normalize_host(url.host_str()?);
        let path_segments: Vec<String> = url
            .path_segments()
            .map(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .map(|segment| segment.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let file_name = path_segments.last().cloned().unwrap_or_default();
        let query_keys = url
            .query_pairs()
            .map(|(key, _)| key.to_ascii_lowercase())
            .collect();
        Some(Self {
            host,
            path_segments,
            file_name,
            query_keys,
        })
    }
}

fn built_in_rules() -> Vec<NetworkRule> {
    let subresources: HashSet<ResourceType> = [
        ResourceType::Script,
        ResourceType::Style,
        ResourceType::Image,
        ResourceType::Font,
        ResourceType::Media,
        ResourceType::Fetch,
        ResourceType::Other,
    ]
    .into_iter()
    .collect();
    let mut rules = Vec::new();

    for label in ["ads", "adserver", "sponsor"] {
        rules.push(NetworkRule {
            _id: format!("builtin-ad-host-{label}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::HostLabel(label.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: false,
        });
    }
    for label in ["tracker", "telemetry"] {
        rules.push(NetworkRule {
            _id: format!("builtin-tracker-host-{label}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::HostLabel(label.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: true,
        });
    }
    for segment in ["ads", "adserver", "sponsored"] {
        rules.push(NetworkRule {
            _id: format!("builtin-ad-path-{segment}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::PathSegment(segment.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: false,
        });
    }
    for segment in ["tracking", "telemetry"] {
        rules.push(NetworkRule {
            _id: format!("builtin-tracker-path-{segment}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::PathSegment(segment.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: true,
        });
    }
    for file_name in ["ads.js", "adframe.js"] {
        rules.push(NetworkRule {
            _id: format!("builtin-ad-file-{file_name}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::FileName(file_name.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: false,
        });
    }
    for file_name in ["tracking.js", "pixel.gif"] {
        rules.push(NetworkRule {
            _id: format!("builtin-tracker-file-{file_name}"),
            action: RuleAction::Block,
            matcher: RuleMatcher::FileName(file_name.to_string()),
            resource_types: subresources.clone(),
            third_party: Some(true),
            page_domains: Vec::new(),
            tracker: true,
        });
    }
    rules
}

fn parse_rule(source: &str, id: String) -> Result<NetworkRule, &'static str> {
    if source.len() > MAX_RULE_BYTES {
        return Err("rule is too long");
    }
    let mut tokens = source.split_whitespace();
    let action = match tokens.next().map(str::to_ascii_uppercase).as_deref() {
        Some("ALLOW") => RuleAction::Allow,
        Some("BLOCK") => RuleAction::Block,
        _ => return Err("rule must start with ALLOW or BLOCK"),
    };

    let mut matcher = None;
    let mut resource_types = HashSet::new();
    let mut third_party = None;
    let mut page_domains = Vec::new();
    let mut tracker = false;
    for token in tokens {
        let (key, value) = token.split_once('=').ok_or("invalid rule token")?;
        let value = value.trim().to_ascii_lowercase();
        match key.to_ascii_lowercase().as_str() {
            "host" => matcher = Some(RuleMatcher::HostSuffix(validate_value(&value)?)),
            "host-label" => matcher = Some(RuleMatcher::HostLabel(validate_value(&value)?)),
            "path" => matcher = Some(RuleMatcher::PathSegment(validate_value(&value)?)),
            "file" => matcher = Some(RuleMatcher::FileName(validate_value(&value)?)),
            "query" => matcher = Some(RuleMatcher::QueryKey(validate_value(&value)?)),
            "types" => {
                for item in value.split(',') {
                    resource_types.insert(parse_resource_type(item)?);
                }
            }
            "third-party" => {
                third_party = Some(match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err("third-party must be true or false"),
                });
            }
            "page" => {
                page_domains = value
                    .split(',')
                    .map(normalize_host)
                    .filter(|domain| !domain.is_empty())
                    .collect();
            }
            "class" => tracker = value == "tracker",
            _ => return Err("unknown rule key"),
        }
    }
    Ok(NetworkRule {
        _id: id,
        action,
        matcher: matcher.ok_or("rule has no matcher")?,
        resource_types,
        third_party,
        page_domains,
        tracker,
    })
}

fn validate_value(value: &str) -> Result<String, &'static str> {
    let value = value.trim_matches(['.', '/']);
    if value.is_empty()
        || value.len() > 253
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("invalid matcher value");
    }
    Ok(value.to_string())
}

fn parse_resource_type(value: &str) -> Result<ResourceType, &'static str> {
    match value {
        "document" => Ok(ResourceType::Document),
        "script" => Ok(ResourceType::Script),
        "style" => Ok(ResourceType::Style),
        "image" => Ok(ResourceType::Image),
        "font" => Ok(ResourceType::Font),
        "media" => Ok(ResourceType::Media),
        "fetch" => Ok(ResourceType::Fetch),
        "other" => Ok(ResourceType::Other),
        _ => Err("unknown resource type"),
    }
}

fn rule_matches(
    rule: &NetworkRule,
    request: &RequestParts,
    page_host: &str,
    resource_type: ResourceType,
    third_party: bool,
) -> bool {
    if !rule.resource_types.is_empty() && !rule.resource_types.contains(&resource_type) {
        return false;
    }
    if rule
        .third_party
        .is_some_and(|expected| expected != third_party)
    {
        return false;
    }
    if !rule.page_domains.is_empty()
        && !rule
            .page_domains
            .iter()
            .any(|domain| host_matches(page_host, domain))
    {
        return false;
    }
    match &rule.matcher {
        RuleMatcher::HostSuffix(domain) => host_matches(&request.host, domain),
        RuleMatcher::HostLabel(label) => request.host.split('.').any(|part| part == label),
        RuleMatcher::PathSegment(segment) => {
            request.path_segments.iter().any(|part| part == segment)
        }
        RuleMatcher::FileName(file_name) => request.file_name == *file_name,
        RuleMatcher::QueryKey(key) => request.query_keys.contains(key),
    }
}

fn normalize_host(source: &str) -> String {
    if let Ok(url) = url::Url::parse(source) {
        return url.host_str().unwrap_or_default().to_ascii_lowercase();
    }
    source
        .trim()
        .trim_matches('.')
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_matches('.');
    !pattern.is_empty() && (host == pattern || host.ends_with(&format!(".{pattern}")))
}

fn same_site(left: &str, right: &str) -> bool {
    host_matches(left, right) || host_matches(right, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_only_matching_third_party_subresources() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        assert!(blocker.should_block_resource(
            "https://ads.cdn.test/banner/creative.js",
            Some("https://news.test/article"),
            ResourceType::Script
        ));
        assert!(!blocker.should_block_resource(
            "https://news.test/ads/guide.html",
            Some("news.test"),
            ResourceType::Document
        ));
        assert!(!blocker.should_block_resource(
            "https://not-ads.test/script.js",
            Some("news.test"),
            ResourceType::Script
        ));
    }

    #[test]
    fn allow_rule_overrides_block_rule_and_updates_stats() {
        let config = AdBlockConfig {
            custom_rules: vec![
                "BLOCK host=cdn.example.test types=image".into(),
                "ALLOW host=cdn.example.test page=shop.test types=image".into(),
            ],
            ..Default::default()
        };
        let mut blocker = AdBlocker::new(config);
        assert!(!blocker.should_block_resource(
            "https://cdn.example.test/photo.jpg",
            Some("shop.test"),
            ResourceType::Image
        ));
        assert_eq!(blocker.stats().allowed_by_exception_count, 1);
    }

    #[test]
    fn domain_toggle_covers_subdomains() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        assert!(!blocker.toggle_domain("example.test".into()));
        assert!(!blocker.is_domain_enabled("www.example.test"));
        assert!(!blocker.should_block_resource(
            "https://ads.cdn.test/creative.js",
            Some("www.example.test"),
            ResourceType::Script
        ));
        assert!(blocker.toggle_domain("example.test".into()));
    }

    #[test]
    fn invalid_custom_rules_are_rejected_without_panicking() {
        let config = AdBlockConfig {
            custom_rules: vec!["COPY-FOREIGN-SYNTAX ||example.test^".into()],
            ..Default::default()
        };
        let blocker = AdBlocker::new(config);
        assert_eq!(blocker.rejected_rule_count(), 1);
    }
}
