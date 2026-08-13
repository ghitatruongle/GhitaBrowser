//! HTTPS-Only Mode & Automatic Security Upgrades for GhitaBrowser (Phase 25).
//! Implements HTTP -> HTTPS automatic URL upgrades and insecure origin warnings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpsMode {
    Disabled,
    EnabledAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsUpgradeResult {
    Upgraded { new_url: String },
    AlreadySecure { url: String },
    ExemptLocal { url: String },
    InsecureAllowed { url: String },
}

pub struct HttpsUpgradeEngine {
    pub mode: HttpsMode,
    pub exemptions: Vec<String>,
}

impl HttpsUpgradeEngine {
    pub fn new(mode: HttpsMode) -> Self {
        Self {
            mode,
            exemptions: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "[::1]".to_string(),
            ],
        }
    }

    pub fn evaluate_url(&self, url: &str) -> HttpsUpgradeResult {
        let parsed = match url::Url::parse(url) {
            Ok(parsed) => parsed,
            Err(_) => {
                return HttpsUpgradeResult::InsecureAllowed {
                    url: url.to_string(),
                }
            }
        };
        if parsed.scheme() != "http" {
            return HttpsUpgradeResult::AlreadySecure {
                url: url.to_string(),
            };
        }

        let domain = parsed.host_str().unwrap_or("");

        if self
            .exemptions
            .iter()
            .any(|ex| domain.eq_ignore_ascii_case(ex.trim_matches(['[', ']'])))
        {
            return HttpsUpgradeResult::ExemptLocal {
                url: url.to_string(),
            };
        }

        match self.mode {
            HttpsMode::Disabled => HttpsUpgradeResult::InsecureAllowed {
                url: url.to_string(),
            },
            HttpsMode::EnabledAll => {
                let mut upgraded = parsed;
                let _ = upgraded.set_scheme("https");
                HttpsUpgradeResult::Upgraded {
                    new_url: upgraded.into(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_upgrade_evaluations() {
        let engine = HttpsUpgradeEngine::new(HttpsMode::EnabledAll);

        // HTTP url upgrades to HTTPS
        assert_eq!(
            engine.evaluate_url("http://example.com/login"),
            HttpsUpgradeResult::Upgraded {
                new_url: "https://example.com/login".to_string()
            }
        );

        // HTTPS url remains secure
        assert_eq!(
            engine.evaluate_url("https://secure.com"),
            HttpsUpgradeResult::AlreadySecure {
                url: "https://secure.com".to_string()
            }
        );

        // Localhost is exempt
        assert_eq!(
            engine.evaluate_url("http://localhost:8080"),
            HttpsUpgradeResult::ExemptLocal {
                url: "http://localhost:8080".to_string()
            }
        );
    }
}
