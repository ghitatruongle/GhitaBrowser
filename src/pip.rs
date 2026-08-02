// Picture-in-Picture state

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipState {
    pub active: bool,
    pub video_url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
}

impl Default for PipState {
    fn default() -> Self {
        Self {
            active: false,
            video_url: String::new(),
            title: String::new(),
            width: 480,
            height: 270,
            always_on_top: true,
        }
    }
}

impl PipState {
    pub fn enable(&mut self, video_url: String, title: String) {
        self.active = true;
        self.video_url = video_url;
        self.title = title;
    }

    pub fn disable(&mut self) {
        self.active = false;
        self.video_url.clear();
        self.title.clear();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(200);
        self.height = height.max(120);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pip_state() {
        let mut pip = PipState::default();
        assert!(!pip.active);

        pip.enable("https://example.com/stream.mp4".to_string(), "Video Title".to_string());
        assert!(pip.active);
        assert_eq!(pip.title, "Video Title");

        pip.resize(640, 360);
        assert_eq!(pip.width, 640);

        pip.disable();
        assert!(!pip.active);
    }
}
