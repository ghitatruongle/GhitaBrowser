// AdBlock and tracker filter engine

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Configuration for AdBlocker engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdBlockConfig {
    pub enabled: bool,
    pub block_trackers: bool,
    pub disabled_domains: HashSet<String>,
}

impl Default for AdBlockConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_trackers: true,
            disabled_domains: HashSet::new(),
        }
    }
}

/// Statistics for blocked requests
#[derive(Debug, Clone, Default)]
pub struct AdBlockStats {
    pub blocked_ads_count: usize,
    pub blocked_trackers_count: usize,
}

/// Core AdBlock engine matching URLs against known ad & tracker patterns
pub struct AdBlocker {
    config: AdBlockConfig,
    stats: AdBlockStats,
    ad_keywords: Vec<&'static str>,
    tracker_keywords: Vec<&'static str>,
}

impl AdBlocker {
    pub fn new(config: AdBlockConfig) -> Self {
        let ad_keywords = vec![
            "/ads/", "/ad/", "/adserver/", "/doubleclick.net/", "/pagead/",
            "googleads.", "adservice.google.", "/popunder", "/popup.",
            "adnxs.com", "rubiconproject.com", "outbrain.com", "taboola.com",
            "-ad-", "_ad_", "/banner/", "/banners/", "ads.js", "adframe.js",
        ];

        let tracker_keywords = vec![
            "analytics.js", "google-analytics.com", "statcounter.com",
            "mixpanel.com", "hotjar.com", "segment.io", "clarity.ms",
            "/pixel.gif", "/tracking", "facebook.com/tr", "telemetry",
        ];

        Self {
            config,
            stats: AdBlockStats::default(),
            ad_keywords,
            tracker_keywords,
        }
    }

    /// Check if a URL should be blocked
    pub fn should_block(&mut self, url: &str, page_domain: Option<&str>) -> bool {
        if !self.config.enabled {
            return false;
        }

        if let Some(domain) = page_domain {
            if self.config.disabled_domains.contains(domain) {
                return false;
            }
        }

        let lower_url = url.to_lowercase();

        // Check ad patterns
        for &pattern in &self.ad_keywords {
            if lower_url.contains(pattern) {
                self.stats.blocked_ads_count += 1;
                return true;
            }
        }

        // Check tracker patterns
        if self.config.block_trackers {
            for &pattern in &self.tracker_keywords {
                if lower_url.contains(pattern) {
                    self.stats.blocked_trackers_count += 1;
                    return true;
                }
            }
        }

        false
    }

    pub fn stats(&self) -> &AdBlockStats {
        &self.stats
    }

    pub fn total_blocked(&self) -> usize {
        self.stats.blocked_ads_count + self.stats.blocked_trackers_count
    }

    pub fn toggle_domain(&mut self, domain: String) -> bool {
        if self.config.disabled_domains.contains(&domain) {
            self.config.disabled_domains.remove(&domain);
            true // Enabled
        } else {
            self.config.disabled_domains.insert(domain);
            false // Disabled
        }
    }

    pub fn is_domain_enabled(&self, domain: &str) -> bool {
        !self.config.disabled_domains.contains(domain)
    }

    pub fn config(&self) -> &AdBlockConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adblock_basic() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        assert!(blocker.should_block("https://googleads.g.doubleclick.net/pagead/ads.js", None));
        assert!(!blocker.should_block("https://example.com/index.html", None));
        assert_eq!(blocker.total_blocked(), 1);
    }

    #[test]
    fn test_adblock_domain_toggle() {
        let mut blocker = AdBlocker::new(AdBlockConfig::default());
        let domain = "example.com";

        assert!(blocker.is_domain_enabled(domain));
        blocker.toggle_domain(domain.to_string());
        assert!(!blocker.is_domain_enabled(domain));
        assert!(!blocker.should_block("https://example.com/ads/banner.js", Some(domain)));
    }
}
