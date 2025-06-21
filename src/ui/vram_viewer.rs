use imgui::{self, TextureId};
use imgui_wgpu::{Renderer, Texture, TextureConfig};
use wgpu::{Extent3d, TextureFormat};

use crate::ppu::Ppu;

#[derive(Copy, Clone, PartialEq, Eq)]
enum VramTab {
    BgMap,
    Tiles,
    Oam,
    Palettes,
}

pub struct VramViewerWindow {
    current: VramTab,
    bg_map_tex: Option<TextureId>,
    last_vblank_frame: u64,
}

impl VramViewerWindow {
    pub fn new() -> Self {
        Self {
            current: VramTab::BgMap,
            bg_map_tex: None,
            last_vblank_frame: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &imgui::Ui,
        ppu: &mut Ppu,
        renderer: &mut Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if let Some(_bar) = imgui::TabBar::new("VRAMTabs").begin(ui) {
            self.tab_item(
                ui,
                renderer,
                ppu,
                device,
                queue,
                VramTab::BgMap,
                "BG Map",
                Self::draw_bg_map,
            );
            self.tab_item(
                ui,
                renderer,
                ppu,
                device,
                queue,
                VramTab::Tiles,
                "Tiles",
                Self::draw_tiles,
            );
            self.tab_item(
                ui,
                renderer,
                ppu,
                device,
                queue,
                VramTab::Oam,
                "OAM",
                Self::draw_oam,
            );
            self.tab_item(
                ui,
                renderer,
                ppu,
                device,
                queue,
                VramTab::Palettes,
                "Palettes",
                Self::draw_palettes,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_item<F>(
        &mut self,
        ui: &imgui::Ui,
        renderer: &mut Renderer,
        ppu: &mut Ppu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        which: VramTab,
        label: &str,
        draw: F,
    ) where
        F: Fn(&mut Self, &imgui::Ui, &mut Renderer, &mut Ppu, &wgpu::Device, &wgpu::Queue),
    {
        if let Some(_tab) = imgui::TabItem::new(label).begin(ui) {
            self.current = which;
            draw(self, ui, renderer, ppu, device, queue);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_bg_map(
        &mut self,
        ui: &imgui::Ui,
        renderer: &mut Renderer,
        ppu: &mut Ppu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let vblank = self.last_vblank_frame.wrapping_add(1);
        if self.bg_map_tex.is_none() || vblank != self.last_vblank_frame {
            self.bg_map_tex = Some(self.build_bg_map_texture(ppu, renderer, device, queue));
            self.last_vblank_frame = vblank;
        }

        let tex_id = self.bg_map_tex.unwrap();
        let size = [256.0, 256.0];
        let avail = ui.content_region_avail();
        let scale = (avail[0] / size[0]).min(2.0);
        let draw_size = [size[0] * scale, size[1] * scale];

        let cursor = ui.cursor_screen_pos();
        imgui::Image::new(tex_id, draw_size).build(ui);

        let scx = ppu.read_reg(0xFF43);
        let scy = ppu.read_reg(0xFF42);
        let tl = [
            cursor[0] + (scx as f32) * scale,
            cursor[1] + (scy as f32) * scale,
        ];
        let br = [tl[0] + 160.0 * scale, tl[1] + 144.0 * scale];

        let draw_list = ui.get_window_draw_list();
        draw_list
            .add_rect(tl, br, imgui::ImColor32::from_rgb(255, 0, 0))
            .thickness(1.0)
            .build();

        if scx > 96 {
            draw_list
                .add_rect(
                    [cursor[0] + ((scx as f32) - 256.0) * scale, tl[1]],
                    [cursor[0] + ((scx as f32) - 96.0) * scale, br[1]],
                    imgui::ImColor32::from_rgb(255, 0, 0),
                )
                .thickness(1.0)
                .build();
        }
        if scy > 112 {
            draw_list
                .add_rect(
                    [tl[0], cursor[1] + ((scy as f32) - 256.0) * scale],
                    [br[0], cursor[1] + ((scy as f32) - 112.0) * scale],
                    imgui::ImColor32::from_rgb(255, 0, 0),
                )
                .thickness(1.0)
                .build();
        }
    }

    fn build_bg_map_texture(
        &self,
        ppu: &mut Ppu,
        renderer: &mut Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> TextureId {
        const MAP_W: usize = 32;
        const MAP_H: usize = 32;
        const TILE: usize = 8;
        const IMG_W: usize = MAP_W * TILE;
        const IMG_H: usize = MAP_H * TILE;
        const DMG_PALETTE: [u32; 4] = [0x009BBC0F, 0x008BAC0F, 0x00306230, 0x000F380F];

        let mut rgba = vec![0u8; IMG_W * IMG_H * 4];

        let lcdc = ppu.read_reg(0xFF40);
        let map_base = if lcdc & 0x08 != 0 { 0x1C00 } else { 0x1800 };
        let tile_data_base = if lcdc & 0x10 != 0 { 0x0000 } else { 0x0800 };
        let signed = lcdc & 0x10 == 0;
        let bgp = ppu.read_reg(0xFF47);

        for tile_y in 0..MAP_H {
            for tile_x in 0..MAP_W {
                let tile_idx = ppu.vram[0][map_base + tile_y * MAP_W + tile_x];
                let tile_num: i16 = if signed {
                    tile_idx as i8 as i16
                } else {
                    tile_idx as i16
                };
                let tile_addr = tile_data_base + tile_num as usize * 16;
                for row in 0..TILE {
                    let lo = ppu.vram[0][tile_addr + row * 2];
                    let hi = ppu.vram[0][tile_addr + row * 2 + 1];
                    for col in 0..TILE {
                        let bit = 7 - col;
                        let idx = ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1);
                        let shade = (bgp >> (idx * 2)) & 0x03;
                        let color = DMG_PALETTE[shade as usize];
                        let x = tile_x * TILE + col;
                        let y = tile_y * TILE + row;
                        let off = (y * IMG_W + x) * 4;
                        rgba[off] = ((color >> 16) & 0xFF) as u8;
                        rgba[off + 1] = ((color >> 8) & 0xFF) as u8;
                        rgba[off + 2] = (color & 0xFF) as u8;
                        rgba[off + 3] = 0xFF;
                    }
                }
            }
        }

        let config = TextureConfig {
            size: Extent3d {
                width: IMG_W as u32,
                height: IMG_H as u32,
                depth_or_array_layers: 1,
            },
            label: Some("BG-Map"),
            format: Some(TextureFormat::Rgba8UnormSrgb),
            ..Default::default()
        };
        let texture = Texture::new(device, renderer, config);
        texture.write(queue, &rgba, IMG_W as u32, IMG_H as u32);
        renderer.textures.insert(texture)
    }

    fn draw_tiles(
        &mut self,
        _ui: &imgui::Ui,
        _r: &mut Renderer,
        _ppu: &mut Ppu,
        _d: &wgpu::Device,
        _q: &wgpu::Queue,
    ) {
    }
    fn draw_oam(
        &mut self,
        _ui: &imgui::Ui,
        _r: &mut Renderer,
        _ppu: &mut Ppu,
        _d: &wgpu::Device,
        _q: &wgpu::Queue,
    ) {
    }
    fn draw_palettes(
        &mut self,
        _ui: &imgui::Ui,
        _r: &mut Renderer,
        _ppu: &mut Ppu,
        _d: &wgpu::Device,
        _q: &wgpu::Queue,
    ) {
    }
}
