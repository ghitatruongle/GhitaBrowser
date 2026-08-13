// Web capture tool

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    FullPage,
    SelectionRegion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RectRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebCaptureState {
    pub active: bool,
    pub mode: CaptureMode,
    pub selection: Option<RectRegion>,
    pub captured_image_data: Option<Vec<u8>>,
}

impl Default for WebCaptureState {
    fn default() -> Self {
        Self {
            active: false,
            mode: CaptureMode::SelectionRegion,
            selection: None,
            captured_image_data: None,
        }
    }
}

impl WebCaptureState {
    pub fn start_capture(&mut self, mode: CaptureMode) {
        self.active = true;
        self.mode = mode;
        self.selection = None;
        self.captured_image_data = None;
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.selection = None;
        self.captured_image_data = None;
    }

    pub fn set_selection(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.selection = Some(RectRegion {
            x,
            y,
            width,
            height,
        });
    }

    pub fn finish_capture(&mut self, fake_png_data: Vec<u8>) {
        self.captured_image_data = Some(fake_png_data);
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_capture_state() {
        let mut capture = WebCaptureState::default();
        assert!(!capture.active);

        capture.start_capture(CaptureMode::SelectionRegion);
        assert!(capture.active);

        capture.set_selection(10.0, 20.0, 300.0, 200.0);
        assert!(capture.selection.is_some());

        capture.finish_capture(vec![1, 2, 3, 4]);
        assert!(!capture.active);
        assert!(capture.captured_image_data.is_some());
    }
}
