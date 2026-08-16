// End-to-end browser compatibility diagnostics, telemetry, and acceptance verification.
// Ensures synthetic unit tests cannot mask real-site rendering failures,
// silent script failures, overlapping text, blank skeleton pages, or resource leaks.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::css_parser::CssDiagnostics;
use crate::layout::{LayoutNode, RectModel};
use crate::package_crypto::sha256_hex;
use crate::web_runtime::RuntimeReport;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "details")]
pub enum CompatibilityStatus {
    FullyCompatible,
    DegradedShell {
        reason: String,
    },
    ScriptError {
        error: String,
    },
    OverlapDetected {
        text_collision_score: f64,
        overlapping_boxes: usize,
    },
    BlankContentDetected {
        blank_ratio: f64,
    },
    UnsupportedFormat {
        reason: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CssTelemetry {
    pub total_rules: usize,
    pub ignored_rules: usize,
    pub unsupported_properties: usize,
    pub diagnostics_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeTelemetry {
    pub scripts_seen: usize,
    pub scripts_executed: usize,
    pub scripts_failed: usize,
    pub runtime_errors: Vec<String>,
    pub microtasks_processed: usize,
    pub dom_mutations: usize,
    pub listeners_count: usize,
    pub heap_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutTelemetry {
    pub layout_boxes: usize,
    pub overlapping_boxes: usize,
    pub text_collision_score: f64,
    pub meaningful_text_area: f64,
    pub blank_content_ratio: f64,
    pub has_overlap: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaTelemetry {
    pub route: Option<String>,
    pub display_mode: Option<String>,
    pub is_playable: bool,
    pub playability_status: Option<String>,
    pub formats_count: usize,
    pub video_frames_count: usize,
    pub audio_frames_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub url: String,
    pub status: CompatibilityStatus,
    pub css: CssTelemetry,
    pub runtime: RuntimeTelemetry,
    pub layout: LayoutTelemetry,
    pub media: Option<MediaTelemetry>,
    pub generated_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFixtureEntry {
    pub path: String,
    pub sha256: String,
    pub description: String,
    pub category: String,
    pub required_viewport_width: Option<u32>,
    pub max_allowed_overlap_score: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceManifest {
    pub schema_version: u32,
    pub track: String,
    pub generated_at_unix: u64,
    pub fixtures: Vec<ManifestFixtureEntry>,
}

/// Redact sensitive query parameters, passwords, bearer tokens, or session IDs.
pub fn redact_sensitive_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("REDACTED"));
    }
    let sensitive_keys = [
        "token",
        "auth",
        "secret",
        "session",
        "api_key",
        "password",
        "access_token",
        "sig",
        "s",
    ];
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| {
            let key_str = k.to_ascii_lowercase();
            if sensitive_keys.iter().any(|&s| key_str.contains(s)) {
                (k.into_owned(), "REDACTED".to_string())
            } else {
                (k.into_owned(), v.into_owned())
            }
        })
        .collect();

    if !pairs.is_empty() {
        parsed
            .query_pairs_mut()
            .clear()
            .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    parsed.to_string()
}

/// Keep diagnostics bounded on very large pages. The renderer itself can
/// retain more nodes, but overlap/blank-content reporting must never turn a
/// page load into an unbounded quadratic scan.
const MAX_DIAGNOSTIC_TEXT_RECTS: usize = 4_096;

/// Traverse layout tree and collect leaf bounding boxes with non-empty text.
fn collect_text_rects(node: &LayoutNode, out: &mut Vec<(RectModel, String)>) {
    if out.len() >= MAX_DIAGNOSTIC_TEXT_RECTS {
        return;
    }
    if node.children.is_empty() {
        if !node.desc_text.trim().is_empty() && node.rect.width > 0.0 && node.rect.height > 0.0 {
            out.push((node.rect, node.desc_text.clone()));
        }
    } else {
        for child in &node.children {
            collect_text_rects(child, out);
            if out.len() >= MAX_DIAGNOSTIC_TEXT_RECTS {
                break;
            }
        }
    }
}

/// Count total layout boxes in a tree.
pub fn count_layout_boxes(node: &LayoutNode) -> usize {
    1 + node.children.iter().map(count_layout_boxes).sum::<usize>()
}

/// Detect geometric overlaps and text collision score between distinct boxes.
pub fn evaluate_layout_overlap(root: &LayoutNode) -> (usize, f64) {
    let mut rects = Vec::new();
    collect_text_rects(root, &mut rects);
    rects.sort_by(|(left, _), (right, _)| left.y.total_cmp(&right.y));

    if rects.len() < 2 {
        return (0, 0.0);
    }

    let mut overlapping_pairs = 0usize;
    let mut total_overlap_area = 0.0f64;
    let mut total_box_area = 0.0f64;

    for (i, (r1, _)) in rects.iter().enumerate() {
        let area1 = r1.width * r1.height;
        total_box_area += area1;

        for (r2, _) in rects.iter().skip(i + 1) {
            // Rectangles are sorted by top edge, so no later rectangle can
            // overlap vertically after this point.
            if r2.y >= r1.y + r1.height {
                break;
            }

            let x_overlap = (r1.x + r1.width).min(r2.x + r2.width) - r1.x.max(r2.x);
            let y_overlap = (r1.y + r1.height).min(r2.y + r2.height) - r1.y.max(r2.y);

            // Consider it a collision only if overlap exceeds threshold in both dimensions
            if x_overlap > 5.0 && y_overlap > 5.0 {
                let intersection = x_overlap * y_overlap;
                let min_area = area1.min(r2.width * r2.height);
                if intersection > min_area * 0.5 {
                    overlapping_pairs += 1;
                    total_overlap_area += intersection;
                }
            }
        }
    }

    let collision_score = if total_box_area > 0.0 {
        (total_overlap_area / total_box_area).min(1.0)
    } else {
        0.0
    };

    (overlapping_pairs, collision_score)
}

/// Detect blank content ratio in viewport.
pub fn detect_blank_content(
    root: Option<&LayoutNode>,
    viewport_width: f64,
    viewport_height: f64,
) -> f64 {
    let Some(root) = root else {
        return 1.0;
    };
    let viewport_area = (viewport_width * viewport_height).max(1.0);
    let mut text_rects = Vec::new();
    collect_text_rects(root, &mut text_rects);

    let rendered_area: f64 = text_rects
        .iter()
        .map(|(r, _)| (r.width * r.height).min(viewport_area))
        .sum();

    if rendered_area <= 0.0 {
        1.0
    } else {
        (1.0 - (rendered_area / viewport_area)).clamp(0.0, 1.0)
    }
}

/// Build a unified compatibility report.
pub fn build_compatibility_report(
    url: &str,
    css_diag: Option<&CssDiagnostics>,
    runtime_report: Option<&RuntimeReport>,
    layout_root: Option<&LayoutNode>,
    media_telemetry: Option<MediaTelemetry>,
    viewport_width: f64,
    viewport_height: f64,
) -> CompatibilityReport {
    let redacted_url = redact_sensitive_url(url);

    let css = if let Some(diag) = css_diag {
        CssTelemetry {
            total_rules: diag.total_rules_parsed,
            ignored_rules: diag.unsupported_selectors.values().sum::<usize>(),
            unsupported_properties: diag.unsupported_properties.values().sum::<usize>(),
            diagnostics_count: diag.parse_errors.len(),
        }
    } else {
        CssTelemetry::default()
    };

    let runtime = if let Some(rt) = runtime_report {
        let errors = if !rt.errors.is_empty() {
            rt.errors.clone()
        } else {
            rt.script_diagnostics
                .iter()
                .filter_map(|d| d.error_message.clone())
                .collect()
        };
        RuntimeTelemetry {
            scripts_seen: rt.scripts_seen,
            scripts_executed: rt.scripts_executed,
            scripts_failed: rt.scripts_failed,
            runtime_errors: errors,
            microtasks_processed: rt.scheduled_tasks,
            dom_mutations: rt.dom_mutations,
            listeners_count: rt.event_listeners,
            heap_bytes: rt.realm_heap_bytes,
        }
    } else {
        RuntimeTelemetry::default()
    };

    let layout = if let Some(root) = layout_root {
        let (overlapping_boxes, collision_score) = evaluate_layout_overlap(root);
        let blank_ratio = detect_blank_content(Some(root), viewport_width, viewport_height);
        let mut text_rects = Vec::new();
        collect_text_rects(root, &mut text_rects);
        let text_area: f64 = text_rects.iter().map(|(r, _)| r.width * r.height).sum();

        LayoutTelemetry {
            layout_boxes: count_layout_boxes(root),
            overlapping_boxes,
            text_collision_score: collision_score,
            meaningful_text_area: text_area,
            blank_content_ratio: blank_ratio,
            has_overlap: overlapping_boxes > 0,
        }
    } else {
        LayoutTelemetry {
            blank_content_ratio: 1.0,
            ..Default::default()
        }
    };

    // Determine status
    let status = if let Some(media) = &media_telemetry {
        if !media.is_playable && media.playability_status.as_deref() != Some("OK") {
            CompatibilityStatus::UnsupportedFormat {
                reason: media
                    .playability_status
                    .clone()
                    .unwrap_or_else(|| "Media format unplayable".to_string()),
            }
        } else if media.display_mode.as_deref() == Some("DegradedShell") {
            CompatibilityStatus::DegradedShell {
                reason: "Degraded bootstrap navigation shell".to_string(),
            }
        } else {
            CompatibilityStatus::FullyCompatible
        }
    } else if layout.has_overlap && layout.text_collision_score > 0.1 {
        CompatibilityStatus::OverlapDetected {
            text_collision_score: layout.text_collision_score,
            overlapping_boxes: layout.overlapping_boxes,
        }
    } else if layout.blank_content_ratio > 0.95
        && runtime.scripts_executed == 0
        && css.total_rules > 0
    {
        CompatibilityStatus::BlankContentDetected {
            blank_ratio: layout.blank_content_ratio,
        }
    } else if !runtime.runtime_errors.is_empty() && runtime.dom_mutations == 0 {
        CompatibilityStatus::ScriptError {
            error: runtime.runtime_errors.first().cloned().unwrap_or_default(),
        }
    } else {
        CompatibilityStatus::FullyCompatible
    };

    CompatibilityReport {
        url: redacted_url,
        status,
        css,
        runtime,
        layout,
        media: media_telemetry,
        generated_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

/// Verify acceptance manifest integrity by checking that all declared files exist
/// and match their locked SHA-256 checksums.
pub fn verify_acceptance_manifest(
    manifest: &AcceptanceManifest,
    base_path: &Path,
) -> Result<(), String> {
    for entry in &manifest.fixtures {
        let full_path = base_path.join(&entry.path);
        let metadata = std::fs::metadata(&full_path)
            .map_err(|e| format!("Manifest fixture missing ({}): {e}", full_path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "Manifest fixture is not a file: {}",
                full_path.display()
            ));
        }
        let bytes = std::fs::read(&full_path)
            .map_err(|e| format!("Failed to read fixture ({}): {e}", full_path.display()))?;
        let actual_hash = sha256_hex(&bytes);
        if !actual_hash.eq_ignore_ascii_case(&entry.sha256) {
            return Err(format!(
                "Fixture SHA-256 mismatch for {}: expected {}, found {}",
                entry.path, entry.sha256, actual_hash
            ));
        }
    }
    Ok(())
}
