// Sidebar panel manager

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarPanel {
    WebApps, // Zalo, Messenger, Facebook web
    Notes,   // Quick notes
    Calculator,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarState {
    pub visible: bool,
    pub active_panel: SidebarPanel,
    pub pinned_apps: Vec<PinnedApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedApp {
    pub name: String,
    pub url: String,
    pub icon_name: String,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: false,
            active_panel: SidebarPanel::WebApps,
            pinned_apps: vec![
                PinnedApp {
                    name: "Zalo".to_string(),
                    url: "https://chat.zalo.me".to_string(),
                    icon_name: "💬".to_string(),
                },
                PinnedApp {
                    name: "Messenger".to_string(),
                    url: "https://messenger.com".to_string(),
                    icon_name: "⚡".to_string(),
                },
                PinnedApp {
                    name: "Notes".to_string(),
                    url: "ghita://notes".to_string(),
                    icon_name: "📝".to_string(),
                },
            ],
        }
    }
}

impl SidebarState {
    pub fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_panel(&mut self, panel: SidebarPanel) {
        self.active_panel = panel;
        self.visible = true;
    }

    pub fn add_pinned_app(&mut self, name: String, url: String, icon_name: String) {
        if !self.pinned_apps.iter().any(|a| a.url == url) {
            self.pinned_apps.push(PinnedApp {
                name,
                url,
                icon_name,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidebar_state() {
        let mut sidebar = SidebarState::default();
        assert!(!sidebar.visible);

        sidebar.toggle_visibility();
        assert!(sidebar.visible);

        sidebar.set_panel(SidebarPanel::Calculator);
        assert_eq!(sidebar.active_panel, SidebarPanel::Calculator);
        assert_eq!(sidebar.pinned_apps.len(), 3);
    }
}
