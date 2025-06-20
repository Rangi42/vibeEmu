use imgui_wgpu::{Renderer, RendererConfig};
use pixels::Pixels;
use winit::window::Window;

/// Wrapper for each editor window
pub struct UiWindow {
    /// OS window handle
    pub win: Window,
    /// 2D framebuffer
    pub pixels: Pixels,
    /// ImGui renderer tied to this window's device
    pub renderer: Renderer,
    /// Type of window
    pub kind: WindowKind,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum WindowKind {
    Debugger,
    VramViewer,
    Main,
}

impl UiWindow {
    /// Create a new UiWindow with its own renderer
    pub fn new(kind: WindowKind, win: Window, pixels: Pixels, imgui: &mut imgui::Context) -> Self {
        let renderer = Renderer::new(
            imgui,
            pixels.device(),
            pixels.queue(),
            RendererConfig {
                texture_format: pixels.render_texture_format(),
                ..Default::default()
            },
        );
        Self {
            win,
            pixels,
            renderer,
            kind,
        }
    }
}
