// Process task manager

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessTaskInfo {
    pub tab_id: usize,
    pub title: String,
    pub url: String,
    pub memory_mb: f32,
    pub cpu_percent: f32,
    pub layout_nodes: usize,
    pub is_incognito: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TaskManager {
    pub open: bool,
    pub tasks: Vec<ProcessTaskInfo>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            open: false,
            tasks: Vec::new(),
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn update_tasks(&mut self, tasks: Vec<ProcessTaskInfo>) {
        self.tasks = tasks;
    }

    pub fn total_memory_mb(&self) -> f32 {
        self.tasks.iter().map(|t| t.memory_mb).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_manager() {
        let mut tm = TaskManager::new();
        assert!(!tm.open);

        tm.toggle();
        assert!(tm.open);

        tm.update_tasks(vec![
            ProcessTaskInfo {
                tab_id: 0,
                title: "Google".to_string(),
                url: "https://google.com".to_string(),
                memory_mb: 45.5,
                cpu_percent: 1.2,
                layout_nodes: 120,
                is_incognito: false,
            },
            ProcessTaskInfo {
                tab_id: 1,
                title: "GitHub".to_string(),
                url: "https://github.com".to_string(),
                memory_mb: 78.0,
                cpu_percent: 2.5,
                layout_nodes: 450,
                is_incognito: false,
            },
        ]);

        assert_eq!(tm.tasks.len(), 2);
        assert!((tm.total_memory_mb() - 123.5).abs() < 0.1);
    }
}
