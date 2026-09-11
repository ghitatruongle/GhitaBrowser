//! HTTPS-Only Mode & Automatic Security Upgrades for GhitaBrowser (Phase 25).
//! Implements HTTP -> HTTPS automatic URL upgrades and insecure origin warnings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpsMode {
    Disabled,
    EnabledAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsUpgradeResult {
    Upgraded {
        new_url: String,
    },
    AlreadySecure {
        url: String,
    },
    /// Schemes that are neither http nor https (file:, ghita:, about:...).
    /// They are not "secure web origins" and must not be labeled as such.
    NonHttpScheme {
        url: String,
    },
    ExemptLocal {
        url: String,
    },
    InsecureAllowed {
        url: String,
    },
    /// The input could not be parsed as a URL. Fail-closed: unlike
    /// `InsecureAllowed`, this signals callers must NOT navigate or fetch
    /// the raw input as an insecure fallback.
    Invalid {
        url: String,
    },
    /// An http URL that was denied rather than upgraded/allowed
    /// (fail-closed for parse failures when callers need an explicit
    /// blocked signal distinct from `Invalid`).
    InsecureBlocked {
        url: String,
    },
}

impl HttpsUpgradeResult {
    /// True for fail-closed outcomes that must not proceed as insecure.
    pub fn is_blocked(&self) -> bool {
        matches!(
            self,
            HttpsUpgradeResult::Invalid { .. } | HttpsUpgradeResult::InsecureBlocked { .. }
        )
    }
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
        // Fail-closed: unparseable input is never reported as
        // `InsecureAllowed` (fail-open). Callers must treat `Invalid` as
        // blocked.
        let parsed = match url::Url::parse(url) {
            Ok(parsed) => parsed,
            Err(_) => {
                return HttpsUpgradeResult::Invalid {
                    url: url.to_string(),
                };
            }
        };
        if parsed.scheme() == "https" {
            return HttpsUpgradeResult::AlreadySecure {
                url: url.to_string(),
            };
        }
        if parsed.scheme() != "http" {
            return HttpsUpgradeResult::NonHttpScheme {
                url: url.to_string(),
            };
        }

        let domain = parsed.host_str().unwrap_or("");
        if is_exempt_local(domain, &self.exemptions) {
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
                // `http://host:80/x` must become `https://host/x`: an
                // explicit :80 is the default for http but a non-default
                // (and wrong) port for https. `Url::set_scheme` preserves
                // the port, so drop it explicitly.
                if upgraded.port() == Some(80) {
                    let _ = upgraded.set_port(None);
                }
                HttpsUpgradeResult::Upgraded {
                    new_url: upgraded.into(),
                }
            }
        }
    }
}

/// Normalize a host for exemption comparison: strip brackets (IPv6
/// `[::1]`), strip a single trailing dot (`localhost.`), trim whitespace
/// and lowercase. Both the request host and each exemption entry are
/// normalized so `[::1]` and `::1` compare equal.
fn normalize_exempt_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches([']', '.'])
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn is_exempt_local(host: &str, exemptions: &[String]) -> bool {
    let normalized = normalize_exempt_host(host);
    if normalized.is_empty() {
        return false;
    }
    // Any loopback IP (127.0.0.1, ::1, 127.x.x.x, ...) is local, even if
    // not listed in `exemptions`. Parsing covers bracket-stripped forms.
    if normalized
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return true;
    }
    if normalized == "localhost" {
        return true;
    }
    exemptions
        .iter()
        .map(|ex| normalize_exempt_host(ex))
        .any(|ex| {
            if ex.is_empty() {
                return false;
            }
            if ex
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            {
                return normalized == ex;
            }
            normalized == ex
        })
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
