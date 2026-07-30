// src/performance.rs - Performance Optimization & Profiling (Phase 21-22)
#![allow(dead_code)]

use std::collections::HashMap;
use super::layout::LayoutNode;

pub struct Profiler {
    timings: HashMap<String, u64>,
}

impl Profiler {
    pub fn new() -> Self {
        Self { timings: HashMap::new() }
    }

    pub fn record(&mut self, name: &str, duration_ms: u64) {
        *self.timings.entry(name.to_string()).or_default() += duration_ms;
    }

    pub fn report(&self) {
        println!("=== Performance Report ===");
        for (name, total) in &self.timings {
            println!("{}: {} ms", name, total);
        }
    }
}

pub fn optimized_layout(root: &mut LayoutNode, viewport_width: u32, _profiler: &Profiler) {
    super::layout::perform_layout(root, viewport_width);
}