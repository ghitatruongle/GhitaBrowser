//! Public Suffix List access shared by cookies, ad blocking and tracking
//! protection.
//!
//! The bundled snapshot lives next to this file
//! (`public_suffix_list.dat`, MPL-2.0, see THIRD_PARTY_NOTICES.md) and is
//! parsed once per process into static sets. Rules follow the PSL algorithm:
//! exception rules win over wildcards, wildcards cover exactly one extra
//! label, and unknown top-level labels fall back to the implicit `*` rule.

use std::collections::HashSet;
use std::sync::OnceLock;

const LIST: &str = include_str!("public_suffix_list.dat");

struct Rules {
    exact: HashSet<&'static str>,
    /// Bases of wildcard rules: `*.ck` stores `ck`.
    wildcard_bases: HashSet<&'static str>,
    /// Exception rules without the leading `!`: `!www.ck` stores `www.ck`.
    exceptions: HashSet<&'static str>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut parsed = Rules {
            exact: HashSet::new(),
            wildcard_bases: HashSet::new(),
            exceptions: HashSet::new(),
        };
        for line in LIST.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            // Only the part before an inline comment counts. List entries are
            // already lowercase and derive from the static string.
            let rule = line.split_whitespace().next().unwrap_or_default();
            if rule.is_empty() {
                continue;
            }
            if let Some(exception) = rule.strip_prefix('!') {
                parsed.exceptions.insert(exception);
            } else if let Some(base) = rule.strip_prefix("*.") {
                parsed.wildcard_bases.insert(base);
            } else {
                parsed.exact.insert(rule);
            }
        }
        parsed
    })
}

/// Number of labels of `host`'s public suffix per the PSL algorithm.
/// Assumes `host` is already lowercase with no trailing dot.
fn public_suffix_label_count(labels: &[&str]) -> usize {
    let n = labels.len();
    // Exception rules take precedence: the candidate itself is NOT the
    // suffix — its parent is.
    for k in (1..=n).rev() {
        let candidate = labels[n - k..].join(".");
        if rules().exceptions.contains(candidate.as_str()) {
            return k - 1;
        }
    }
    // Longest matching rule wins.
    for k in (1..=n).rev() {
        let candidate = labels[n - k..].join(".");
        if rules().exact.contains(candidate.as_str()) {
            return k;
        }
        // A wildcard rule covers exactly one label above its base.
        if k >= 2
            && rules()
                .wildcard_bases
                .contains(labels[n - k + 1..].join(".").as_str())
        {
            return k;
        }
    }
    // Implicit rule: every top-level label is a public suffix.
    1
}

fn normalized_labels(domain: &str) -> Option<Vec<&str>> {
    let domain = domain.trim().trim_end_matches('.');
    if domain.is_empty() {
        return None;
    }
    Some(
        domain
            .split('.')
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .collect(),
    )
}

/// True when `domain` is itself a public suffix (e.g. `com`, `co.uk`,
/// `github.io`). Unknown single-label TLDs count as public suffixes.
pub fn is_public_suffix(domain: &str) -> bool {
    let Some(labels) = normalized_labels(domain) else {
        return false;
    };
    let lowered: Vec<String> = labels.iter().map(|l| l.to_ascii_lowercase()).collect();
    let refs: Vec<&str> = lowered.iter().map(String::as_str).collect();
    public_suffix_label_count(&refs) == refs.len()
}

/// The registrable domain (eTLD+1) of `host`, or `None` when the host is
/// entirely a public suffix. Falls back to `None` rather than guessing so
/// callers can fail closed.
pub fn registrable_domain(host: &str) -> Option<String> {
    let labels = normalized_labels(host)?;
    let lowered: Vec<String> = labels.iter().map(|l| l.to_ascii_lowercase()).collect();
    let refs: Vec<&str> = lowered.iter().map(String::as_str).collect();
    let suffix_labels = public_suffix_label_count(&refs);
    if suffix_labels >= refs.len() {
        return None;
    }
    Some(refs[refs.len() - suffix_labels - 1..].join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_suffixes_are_recognized() {
        assert!(is_public_suffix("com"));
        assert!(is_public_suffix("co.uk"));
        assert!(is_public_suffix("github.io"));
        assert!(is_public_suffix("pages.dev"));
        assert!(!is_public_suffix("example.com"));
        assert!(!is_public_suffix("user.github.io"));
    }

    #[test]
    fn wildcard_and_exception_rules_behave_per_spec() {
        // *.ck: every first-level subdomain of ck is a public suffix...
        assert!(is_public_suffix("foo.ck"));
        // ...and the exception carves www.ck back out.
        assert!(!is_public_suffix("www.ck"));
        assert_eq!(registrable_domain("www.ck").as_deref(), Some("www.ck"));
        assert_eq!(
            registrable_domain("site.foo.ck").as_deref(),
            Some("site.foo.ck")
        );
    }

    #[test]
    fn registrable_domains_follow_etld_plus_one() {
        assert_eq!(
            registrable_domain("a.b.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(
            registrable_domain("user1.github.io").as_deref(),
            Some("user1.github.io")
        );
        assert_ne!(
            registrable_domain("user1.github.io"),
            registrable_domain("user2.github.io")
        );
        // Entirely a public suffix.
        assert_eq!(registrable_domain("co.uk"), None);
        assert_eq!(registrable_domain("github.io"), None);
    }
}
