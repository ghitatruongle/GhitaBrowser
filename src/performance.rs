//! Bounded performance telemetry and release-budget evaluation.

use super::layout::LayoutNode;
use std::collections::{HashMap, VecDeque};

const MAX_SAMPLES_PER_PHASE: usize = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseSnapshot {
    pub total_ms: u64,
    pub sample_count: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Default)]
struct PhaseSamples {
    total_ms: u64,
    samples: VecDeque<u64>,
}

#[derive(Debug, Default)]
pub struct Profiler {
    phases: HashMap<String, PhaseSamples>,
}

impl Profiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, name: &str, duration_ms: u64) {
        let phase = self.phases.entry(name.to_string()).or_default();
        phase.total_ms = phase.total_ms.saturating_add(duration_ms);
        if phase.samples.len() >= MAX_SAMPLES_PER_PHASE {
            phase.samples.pop_front();
        }
        phase.samples.push_back(duration_ms);
    }

    pub fn snapshot(&self, name: &str) -> Option<PhaseSnapshot> {
        let phase = self.phases.get(name)?;
        let mut ordered: Vec<u64> = phase.samples.iter().copied().collect();
        ordered.sort_unstable();
        Some(PhaseSnapshot {
            total_ms: phase.total_ms,
            sample_count: ordered.len(),
            p50_ms: percentile(&ordered, 50),
            p95_ms: percentile(&ordered, 95),
            max_ms: ordered.last().copied().unwrap_or_default(),
        })
    }

    pub fn phase_count(&self) -> usize {
        self.phases.len()
    }

    pub fn report(&self) {
        #[cfg(debug_assertions)]
        {
            println!("=== Performance Report ===");
            let mut names: Vec<&str> = self.phases.keys().map(String::as_str).collect();
            names.sort_unstable();
            for name in names {
                if let Some(snapshot) = self.snapshot(name) {
                    println!(
                        "{name}: total={}ms samples={} p50={}ms p95={}ms max={}ms",
                        snapshot.total_ms,
                        snapshot.sample_count,
                        snapshot.p50_ms,
                        snapshot.p95_ms,
                        snapshot.max_ms
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavigationMetrics {
    pub fetch_ms: u64,
    pub parse_ms: u64,
    pub style_ms: u64,
    pub layout_ms: u64,
    pub render_ms: u64,
    pub total_ms: u64,
    pub dom_nodes: usize,
    pub estimated_memory_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceBudget {
    pub max_fetch_ms: u64,
    pub max_parse_ms: u64,
    pub max_style_ms: u64,
    pub max_layout_ms: u64,
    pub max_render_ms: u64,
    pub max_total_ms: u64,
    pub max_dom_nodes: usize,
    pub max_document_memory_bytes: usize,
}

impl Default for PerformanceBudget {
    fn default() -> Self {
        Self {
            max_fetch_ms: 10_000,
            max_parse_ms: 250,
            max_style_ms: 250,
            max_layout_ms: 400,
            max_render_ms: 250,
            max_total_ms: 1_500,
            max_dom_nodes: 20_000,
            max_document_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BudgetEvaluation {
    pub violations: Vec<String>,
}

/// Per-frame budget for the retained dynamic renderer. This is separate from
/// navigation timing: a mutation frame must not inherit the network budget of
/// the document that originally loaded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicFrameBudget {
    pub max_frame_time_ms: u64,
    pub max_retained_bytes: usize,
    pub max_active_animations: usize,
}

impl Default for DynamicFrameBudget {
    fn default() -> Self {
        Self {
            max_frame_time_ms: 32,
            max_retained_bytes: 32 * 1024 * 1024,
            max_active_animations: 4_096,
        }
    }
}

impl DynamicFrameBudget {
    pub fn evaluate(
        &self,
        metrics: &crate::dynamic_render::DynamicRenderMetrics,
    ) -> BudgetEvaluation {
        let mut violations = Vec::new();
        check_budget(
            &mut violations,
            "frame_time_ms",
            metrics.frame_time_ms,
            self.max_frame_time_ms,
        );
        check_budget(
            &mut violations,
            "retained_bytes",
            metrics.estimated_retained_bytes as u64,
            self.max_retained_bytes as u64,
        );
        check_budget(
            &mut violations,
            "active_animations",
            metrics.active_animations as u64,
            self.max_active_animations as u64,
        );
        BudgetEvaluation { violations }
    }
}

impl BudgetEvaluation {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

impl PerformanceBudget {
    pub fn evaluate(&self, metrics: NavigationMetrics) -> BudgetEvaluation {
        let mut violations = Vec::new();
        check_budget(
            &mut violations,
            "fetch_ms",
            metrics.fetch_ms,
            self.max_fetch_ms,
        );
        check_budget(
            &mut violations,
            "parse_ms",
            metrics.parse_ms,
            self.max_parse_ms,
        );
        check_budget(
            &mut violations,
            "style_ms",
            metrics.style_ms,
            self.max_style_ms,
        );
        check_budget(
            &mut violations,
            "layout_ms",
            metrics.layout_ms,
            self.max_layout_ms,
        );
        check_budget(
            &mut violations,
            "render_ms",
            metrics.render_ms,
            self.max_render_ms,
        );
        check_budget(
            &mut violations,
            "total_ms",
            metrics.total_ms,
            self.max_total_ms,
        );
        check_budget(
            &mut violations,
            "dom_nodes",
            metrics.dom_nodes as u64,
            self.max_dom_nodes as u64,
        );
        check_budget(
            &mut violations,
            "document_memory_bytes",
            metrics.estimated_memory_bytes as u64,
            self.max_document_memory_bytes as u64,
        );
        BudgetEvaluation { violations }
    }
}

fn percentile(ordered: &[u64], percentile: usize) -> u64 {
    if ordered.is_empty() {
        return 0;
    }
    let index = ((ordered.len() - 1) * percentile).div_ceil(100);
    ordered[index]
}

fn check_budget(violations: &mut Vec<String>, name: &str, actual: u64, maximum: u64) {
    if actual > maximum {
        violations.push(format!("{name}={actual} exceeds {maximum}"));
    }
}

pub fn optimized_layout(root: &mut LayoutNode, viewport_width: u32, _profiler: &Profiler) {
    super::layout::perform_layout(root, viewport_width as f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiler_reports_percentiles_and_bounds_samples() {
        let mut profiler = Profiler::new();
        for value in 1..=600 {
            profiler.record("layout", value);
        }
        let snapshot = profiler.snapshot("layout").unwrap();
        assert_eq!(snapshot.sample_count, MAX_SAMPLES_PER_PHASE);
        assert_eq!(snapshot.max_ms, 600);
        assert!(snapshot.p95_ms >= snapshot.p50_ms);
        assert_eq!(profiler.phase_count(), 1);
    }

    #[test]
    fn performance_budget_names_every_regression() {
        let budget = PerformanceBudget::default();
        let evaluation = budget.evaluate(NavigationMetrics {
            fetch_ms: 1,
            parse_ms: budget.max_parse_ms + 1,
            style_ms: 1,
            layout_ms: 1,
            render_ms: 1,
            total_ms: 1,
            dom_nodes: budget.max_dom_nodes + 1,
            estimated_memory_bytes: 1,
        });
        assert!(!evaluation.passed());
        assert_eq!(evaluation.violations.len(), 2);
        assert!(evaluation.violations[0].contains("parse_ms"));
        assert!(evaluation.violations[1].contains("dom_nodes"));
    }

    #[test]
    fn dynamic_frame_budget_reports_frame_and_memory_regressions() {
        let budget = DynamicFrameBudget::default();
        let evaluation = budget.evaluate(&crate::dynamic_render::DynamicRenderMetrics {
            frame_time_ms: budget.max_frame_time_ms + 1,
            estimated_retained_bytes: budget.max_retained_bytes + 1,
            ..Default::default()
        });
        assert!(!evaluation.passed());
        assert_eq!(evaluation.violations.len(), 2);
    }
}
