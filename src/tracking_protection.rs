//! Third-Party Cookie & Anti-Fingerprinting Protection for GhitaBrowser (Phase 25).
//! Implements cross-site cookie isolation, canvas fingerprint noise injection, and UA masking.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePolicy {
    AllowAll,
    BlockThirdParty,
    BlockAll,
}

pub struct ThirdPartyCookieBlocker {
    pub policy: CookiePolicy,
}

impl ThirdPartyCookieBlocker {
    pub fn new(policy: CookiePolicy) -> Self {
        Self { policy }
    }

    pub fn should_allow_cookie(&self, top_level_domain: &str, cookie_domain: &str) -> bool {
        match self.policy {
            CookiePolicy::AllowAll => true,
            CookiePolicy::BlockAll => false,
            CookiePolicy::BlockThirdParty => {
                let top = url::Url::parse(top_level_domain)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
                let cookie = cookie_domain
                    .trim_start_matches('.')
                    .trim_end_matches('.')
                    .to_ascii_lowercase();
                top.is_some_and(|top| domain_matches(&top, &cookie))
            }
        }
    }
}

fn domain_matches(host: &str, cookie_domain: &str) -> bool {
    !cookie_domain.is_empty()
        && (host == cookie_domain
            || host
                .strip_suffix(cookie_domain)
                .is_some_and(|prefix| prefix.ends_with('.')))
}

pub struct CanvasFingerprintProtector {
    pub enabled: bool,
    pub noise_seed: u32,
}

impl CanvasFingerprintProtector {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            noise_seed: 0x1337_c0de,
        }
    }

    /// Apply deterministic +/-1 LSB noise to a canvas RGBA pixel buffer.
    pub fn scramble_pixel_buffer(&mut self, rgba_buffer: &mut [u8]) {
        if !self.enabled {
            return;
        }

        for (i, byte) in rgba_buffer.iter_mut().enumerate() {
            // Apply subtle noise to R, G, B channels (leave Alpha untouched)
            if i % 4 != 3 {
                self.noise_seed = self
                    .noise_seed
                    .wrapping_mul(1664525)
                    .wrapping_add(1013904223);
                let noise = ((self.noise_seed >> 24) % 3) as i8 - 1; // -1, 0, or +1
                *byte = (*byte as i16 + noise as i16).clamp(0, 255) as u8;
            }
        }
    }

    /// Per-process secret mixed into per-origin seeds. Without it the seed
    /// derives only from the public origin string, so any tracker could
    /// recompute the exact noise offline and recover the pristine canvas —
    /// and the noised value became a stable cross-session identifier.
    fn process_secret() -> u32 {
        static SECRET: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *SECRET.get_or_init(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let stack_addr = &seed as *const u64 as u64;
            use std::hash::{BuildHasher, Hasher};
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u64(stack_addr);
            let mixed = (hasher.finish() as u32) ^ (seed as u32);
            if mixed == 0 {
                0x9E37_79B9
            } else {
                mixed
            }
        })
    }

    pub fn for_origin(enabled: bool, origin: &str) -> Self {
        let mut seed = Self::process_secret();
        for byte in origin.as_bytes() {
            seed ^= u32::from(*byte);
            seed = seed.wrapping_mul(0x0100_0193);
        }
        Self {
            enabled,
            noise_seed: seed,
        }
    }
}

pub struct UserAgentMasker;

impl UserAgentMasker {
    pub fn get_masked_user_agent() -> &'static str {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) GhitaBrowser/2.0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_cookie_blocking_policy() {
        let blocker = ThirdPartyCookieBlocker::new(CookiePolicy::BlockThirdParty);

        // Same domain is allowed
        assert!(blocker.should_allow_cookie("https://example.com", "example.com"));
        assert!(blocker.should_allow_cookie("https://sub.example.com", "example.com"));

        // Cross-domain third party is blocked
        assert!(!blocker.should_allow_cookie("https://example.com", "tracker.adtech.com"));
    }

    #[test]
    fn canvas_fingerprint_noise_scrambling() {
        let mut protector = CanvasFingerprintProtector::new(true);
        let mut pixels = vec![100, 150, 200, 255, 50, 60, 70, 255];
        let original_alpha1 = pixels[3];
        let original_alpha2 = pixels[7];

        protector.scramble_pixel_buffer(&mut pixels);

        // Alpha channels remain untouched
        assert_eq!(pixels[3], original_alpha1);
        assert_eq!(pixels[7], original_alpha2);
    }
}
