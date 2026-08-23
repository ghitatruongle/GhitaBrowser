//! GhitaBrowser's clean-room request and cosmetic filtering engine.
//!
//! The rule grammar is intentionally small and project-specific:
//! `BLOCK host-label=ads types=script,image third-party=true`
//! `ALLOW host=static.example.test page=example.test`

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const MAX_CUSTOM_RULES: usize = 4_096;
const MAX_RULE_BYTES: usize = 2_048;
const SAFE_COSMETIC_SELECTORS: [&str; 3] = [
    "[data-ghita-ad]",
    "[aria-label='Advertisement']",
    ".sponsored-content",
];

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdBlockReason {
    Disabled,
    SiteException,
    SafetyBypass,
    AllowRule(String),
    NoMatch,
    BuiltInRule(String),
    CustomRule(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdBlockDecision {
    pub blocked: bool,
    pub reason: AdBlockReason,
}

impl AdBlockDecision {
    fn allow(reason: AdBlockReason) -> Self {
        Self {
            blocked: false,
            reason,
        }
    }

    fn block(reason: AdBlockReason) -> Self {
        Self {
            blocked: true,
            reason,
        }
    }
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
    id: String,
    built_in: bool,
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
            match parse_rule(source, format!("custom-{index}"), false) {
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
        self.evaluate_resource(url, page_url_or_domain, resource_type)
            .blocked
    }

    pub fn evaluate_resource(
        &mut self,
        url: &str,
        page_url_or_domain: Option<&str>,
        resource_type: ResourceType,
    ) -> AdBlockDecision {
        if !self.config.enabled {
            return AdBlockDecision::allow(AdBlockReason::Disabled);
        }

        let page_host = page_url_or_domain.map(normalize_host).unwrap_or_default();
        if !page_host.is_empty() && self.is_disabled_for_host(&page_host) {
            self.stats.allowed_by_exception_count =
                self.stats.allowed_by_exception_count.saturating_add(1);
            return AdBlockDecision::allow(AdBlockReason::SiteException);
        }

        // Without a trustworthy top-level context there is no safe way to
        // establish third-party status. Top-level documents and local URLs
        // are always allowed, including explicit custom BLOCK rules.
        if resource_type == ResourceType::Document || page_host.is_empty() {
            return AdBlockDecision::allow(AdBlockReason::SafetyBypass);
        }

        let Some(request) = RequestParts::parse(url) else {
            return AdBlockDecision::allow(AdBlockReason::SafetyBypass);
        };
        self.stats.evaluated_count = self.stats.evaluated_count.saturating_add(1);
        if is_local_or_loopback(&request.host) || same_site(&request.host, &page_host) {
            return AdBlockDecision::allow(AdBlockReason::SafetyBypass);
        }
        let third_party = true;

        // Explicit ALLOW rules have priority so users can recover a site from
        // a false positive without disabling protection for the whole site.
        if let Some(rule) = self.rules.iter().find(|rule| {
            rule.action == RuleAction::Allow
                && rule_matches(rule, &request, &page_host, resource_type, third_party)
        }) {
            let id = rule.id.clone();
            self.stats.allowed_by_exception_count =
                self.stats.allowed_by_exception_count.saturating_add(1);
            return AdBlockDecision::allow(AdBlockReason::AllowRule(id));
        }

        let matched = self.rules.iter().find(|rule| {
            rule.action == RuleAction::Block
                && (self.config.block_trackers || !rule.tracker)
                && rule_matches(rule, &request, &page_host, resource_type, third_party)
        });
        let Some(rule) = matched else {
            return AdBlockDecision::allow(AdBlockReason::NoMatch);
        };

        let rule_id = rule.id.clone();
        let built_in = rule.built_in;
        let tracker = rule.tracker;

        if tracker {
            self.stats.blocked_trackers_count = self.stats.blocked_trackers_count.saturating_add(1);
        } else {
            self.stats.blocked_ads_count = self.stats.blocked_ads_count.saturating_add(1);
        }
        let count = self
            .stats
            .blocked_by_resource
            .entry(resource_type)
            .or_default();
        *count = count.saturating_add(1);
        let reason = if built_in {
            AdBlockReason::BuiltInRule(rule_id)
        } else {
            AdBlockReason::CustomRule(rule_id)
        };
        AdBlockDecision::block(reason)
    }

    pub fn cosmetic_selectors(&self, page_url_or_domain: &str) -> Vec<&'static str> {
        let host = normalize_host(page_url_or_domain);
        if !self.config.enabled
            || !self.config.cosmetic_filtering
            || self.is_disabled_for_host(&host)
        {
            return Vec::new();
        }
        SAFE_COSMETIC_SELECTORS.to_vec()
    }

    pub fn stats(&self) -> &AdBlockStats {
        &self.stats
    }

    pub fn total_blocked(&self) -> usize {
        self.stats.blocked_ads_count + self.stats.blocked_trackers_count
    }

    pub fn toggle_domain(&mut self, domain: String) -> bool {
        let enabled = !self.is_domain_enabled(&domain);
        let _ = self.set_domain_enabled(&domain, enabled);
        enabled
    }

    /// Idempotently enable or disable request filtering for a site.
    /// Returns true only when the configuration changed.
    pub fn set_domain_enabled(&mut self, domain: &str, enabled: bool) -> bool {
        let domain = normalize_host(domain);
        if domain.is_empty() {
            return false;
        }
        let was_enabled = self.is_domain_enabled(&domain);
        if enabled {
            self.config.disabled_domains.remove(&domain);
        } else {
            self.config.disabled_domains.insert(domain);
        }
        was_enabled != enabled
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
        ResourceType::Image,
        ResourceType::Fetch,
        ResourceType::Other,
    ]
    .into_iter()
    .collect();
    let mut rules = Vec::new();

    for label in ["ads", "adserver", "sponsor"] {
        rules.push(NetworkRule {
            id: format!("builtin-ad-host-{label}"),
            built_in: true,
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
            id: format!("builtin-tracker-host-{label}"),
            built_in: true,
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
            id: format!("builtin-ad-path-{segment}"),
            built_in: true,
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
            id: format!("builtin-tracker-path-{segment}"),
            built_in: true,
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
            id: format!("builtin-ad-file-{file_name}"),
            built_in: true,
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
            id: format!("builtin-tracker-file-{file_name}"),
            built_in: true,
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

fn parse_rule(source: &str, id: String, built_in: bool) -> Result<NetworkRule, &'static str> {
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
        id,
        built_in,
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
    if host_matches(left, right) || host_matches(right, left) {
        return true;
    }
    registrable_domain(left) == registrable_domain(right)
}

fn registrable_domain(host: &str) -> String {
    // eTLD+1 via the bundled Public Suffix List so distinct tenants on
    // shared suffixes (user1.github.io vs user2.github.io) are NOT same-site.
    // Falls back to the host itself when no registrable part exists.
    crate::public_suffix::registrable_domain(host)
        .unwrap_or_else(|| host.to_ascii_lowercase())
}

fn is_local_or_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
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

    #[test]
    fn safety_bypass_never_blocks_documents_local_or_same_site_requests() {
        let mut blocker = AdBlocker::new(AdBlockConfig {
            custom_rules: vec![
                "BLOCK host-label=ads types=document,script,style,font,media,image,fetch,other"
                    .into(),
            ],
            ..AdBlockConfig::default()
        });

        for (url, page, kind) in [
            (
                "https://ads.test/page",
                Some("https://site.test"),
                ResourceType::Document,
            ),
            (
                "http://127.0.0.1/ads.js",
                Some("http://127.0.0.1"),
                ResourceType::Script,
            ),
            (
                "file:///C:/tmp/ads.js",
                Some("file:///C:/tmp/index.html"),
                ResourceType::Script,
            ),
            (
                "https://cdn.site.test/ads.js",
                Some("https://www.site.test"),
                ResourceType::Script,
            ),
            ("https://ads.test/ads.js", None, ResourceType::Script),
        ] {
            assert!(!blocker.evaluate_resource(url, page, kind).blocked, "{url}");
        }
    }

    #[test]
    fn built_in_tracker_rules_only_block_safe_third_party_resource_types() {
        for kind in [
            ResourceType::Script,
            ResourceType::Image,
            ResourceType::Fetch,
            ResourceType::Other,
        ] {
            let mut blocker = AdBlocker::new(AdBlockConfig::default());
            assert!(blocker.should_block_resource(
                "https://tracker.other.test/collect",
                Some("https://site.test"),
                kind,
            ));
        }
        for kind in [
            ResourceType::Document,
            ResourceType::Style,
            ResourceType::Font,
            ResourceType::Media,
        ] {
            let mut blocker = AdBlocker::new(AdBlockConfig::default());
            assert!(!blocker.should_block_resource(
                "https://tracker.other.test/collect",
                Some("https://site.test"),
                kind,
            ));
        }
    }

    #[test]
    fn ordinary_words_that_contain_ad_are_not_ads() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        for url in [
            "https://cdn.other.test/download/adapter.js",
            "https://not-ads.test/assets/gradient.png",
            "https://cdn.other.test/assets/header.js",
        ] {
            assert!(!blocker.should_block_resource(
                url,
                Some("https://site.test"),
                ResourceType::Script,
            ));
        }
    }

    #[test]
    fn site_enabled_setter_is_idempotent_and_reversible() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        assert!(blocker.set_domain_enabled("example.test", false));
        assert!(!blocker.set_domain_enabled("example.test", false));
        assert!(!blocker.is_domain_enabled("www.example.test"));
        assert!(blocker.set_domain_enabled("example.test", true));
        assert!(blocker.is_domain_enabled("www.example.test"));
    }
}
