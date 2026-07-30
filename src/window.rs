// src/window.rs - Native Window Manager using winit for Phase 13-14
#![allow(dead_code)]

use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Icon, WindowBuilder};

pub struct WindowManager {
    title: String,
    width: u32,
    height: u32,
}

impl WindowManager {
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(WindowManager {
            title: title.to_string(),
            width,
            height,
        })
    }

    pub fn run_event_loop(self) -> Result<(), Box<dyn std::error::Error>> {
        let event_loop = EventLoop::new()?;
        let mut builder = WindowBuilder::new()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(self.width as f64, self.height as f64));

        if let Ok(icon) = load_window_icon() {
            builder = builder.with_window_icon(Some(icon));
        }

        let _window = builder.build(&event_loop)?;

        println!("🌐 GhitaBrowser window running. Close window to exit.");

        event_loop.run(move |event, target| {
            target.set_control_flow(ControlFlow::Wait);

            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    target.exit();
                }
                _ => ()
            }
        })?;

        Ok(())
    }
}

fn load_window_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let img_path = if std::path::Path::new("logo.png").exists() {
        "logo.png"
    } else {
        "icon.ico"
    };

    let img = image::open(img_path)?.to_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();

    let icon = Icon::from_rgba(rgba, width, height)?;
    Ok(icon)
}