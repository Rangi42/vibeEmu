use crate::ui::vram_viewer::VramViewerWindow;
use imgui_wgpu::{Renderer, RendererConfig};
use log::warn;
use pixels::Pixels;
use winit::{dpi::PhysicalSize, window::Window};

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
    /// Optional VRAM viewer state
    pub vram_viewer: Option<VramViewerWindow>,
    buffer_width: u32,
    buffer_height: u32,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum WindowKind {
    Debugger,
    VramViewer,
    Main,
}

impl UiWindow {
    /// Create a new UiWindow with its own renderer
    pub fn new(
        kind: WindowKind,
        win: Window,
        pixels: Pixels,
        buffer_size: (u32, u32),
        imgui: &mut imgui::Context,
    ) -> Self {
        let renderer = Renderer::new(
            imgui,
            pixels.device(),
            pixels.queue(),
            RendererConfig {
                texture_format: pixels.render_texture_format(),
                ..Default::default()
            },
        );
        let vram_viewer = if matches!(kind, WindowKind::VramViewer) {
            Some(VramViewerWindow::new())
        } else {
            None
        };
        Self {
            win,
            pixels,
            renderer,
            kind,
            vram_viewer,
            buffer_width: buffer_size.0,
            buffer_height: buffer_size.1,
        }
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        if let Err(err) = self.pixels.resize_surface(new_size.width, new_size.height) {
            warn!(
                "Failed to resize surface for window {:?}: {err}",
                self.win.id()
            );
        }
        if let Err(err) = self
            .pixels
            .resize_buffer(self.buffer_width, self.buffer_height)
        {
            warn!(
                "Failed to resize pixel buffer for window {:?}: {err}",
                self.win.id()
            );
        }
    }
}
