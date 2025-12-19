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
    surface_width: u32,
    surface_height: u32,
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
            // Unknown until we've successfully called Pixels::resize_surface.
            // We intentionally start at 0 so the first redraw can force-sync.
            surface_width: 0,
            surface_height: 0,
        }
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_width, self.surface_height)
    }

    pub fn ensure_surface_matches_window(&mut self) {
        let size = self.win.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        if size.width != self.surface_width || size.height != self.surface_height {
            self.resize(size);
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
        } else {
            self.surface_width = new_size.width;
            self.surface_height = new_size.height;
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
