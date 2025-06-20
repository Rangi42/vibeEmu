#![allow(dead_code)]

mod apu;
mod cartridge;
mod cpu;
mod gameboy;
mod input;
mod mmu;
mod ppu;
mod serial;
mod timer;

use clap::Parser;
use imgui::{ConfigFlags, Context as ImguiContext};
use imgui_wgpu::{Renderer, RendererConfig};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use log::info;
use pixels::{Pixels, SurfaceTexture};
use std::sync::Arc;
use std::time::Duration;
use winit::dpi::PhysicalPosition;
use winit::{
    event::MouseButton,
    event::{ElementState, Event, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

const SCALE: u32 = 3;

#[derive(Default)]
struct UiState {
    paused: bool,
    show_context: bool,
    ctx_pos: [f32; 2],
    show_debugger: bool,
    show_vram: bool,
}

#[derive(Parser)]
struct Args {
    /// Path to ROM file
    rom: Option<std::path::PathBuf>,

    /// Force DMG mode
    #[arg(long, conflicts_with = "cgb")]
    dmg: bool,

    /// Force CGB mode
    #[arg(long, conflicts_with = "dmg")]
    cgb: bool,

    /// Run in serial test mode
    #[arg(long)]
    serial: bool,

    /// Path to boot ROM file
    #[arg(long)]
    bootrom: Option<std::path::PathBuf>,

    /// Enable debug logging of CPU state and serial output
    #[arg(long)]
    debug: bool,

    /// Run without opening a window
    #[arg(long)]
    headless: bool,

    /// Number of frames to run in headless mode
    #[arg(long)]
    frames: Option<usize>,

    /// Number of seconds to run in headless mode
    #[arg(long)]
    seconds: Option<u64>,

    /// Number of CPU cycles to run in headless mode
    #[arg(long)]
    cycles: Option<u64>,
}

fn cursor_in_screen(window: &winit::window::Window, pos: PhysicalPosition<f64>) -> bool {
    let size = window.inner_size();
    let width = (160 * SCALE) as f64;
    let height = (144 * SCALE) as f64;
    let x_in = pos.x >= 0.0 && pos.x < width.min(size.width as f64);
    let y_in = pos.y >= 0.0 && pos.y < height.min(size.height as f64);
    x_in && y_in
}

fn build_ui(_state: &mut UiState, _ui: &imgui::Ui) {
    // Placeholder for UI rendering
}

fn main() {
    env_logger::init();
    let args = Args::parse();

    info!("Starting emulator");

    let rom_path = match args.rom {
        Some(p) => p,
        None => {
            eprintln!("No ROM supplied");
            return;
        }
    };

    let cart = match cartridge::Cartridge::from_file(&rom_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load ROM: {e}");
            return;
        }
    };

    let cgb_mode = if args.dmg {
        false
    } else if args.cgb {
        true
    } else {
        cart.cgb
    };
    let mut gb = gameboy::GameBoy::new_with_mode(cgb_mode);
    gb.mmu.load_cart(cart);

    if let Some(path) = args.bootrom {
        match std::fs::read(&path) {
            Ok(data) => gb.mmu.load_boot_rom(data),
            Err(e) => eprintln!("Failed to load boot ROM: {e}"),
        }
    }

    println!(
        "Emulator initialized in {} mode",
        if cgb_mode { "CGB" } else { "DMG" }
    );

    let _stream = if args.headless {
        None
    } else {
        apu::Apu::start_stream(Arc::clone(&gb.mmu.apu))
    };

    let mut frame = vec![0u32; 160 * 144];
    let mut frame_count = 0u64;
    let mut ui_state = UiState::default();

    if !args.headless {
        let event_loop = EventLoop::new();
        let window = WindowBuilder::new()
            .with_title("vibeEmu")
            .with_inner_size(winit::dpi::LogicalSize::new(
                (160 * SCALE) as f64,
                (144 * SCALE) as f64,
            ))
            .build(&event_loop)
            .expect("Failed to create window");

        let size = window.inner_size();
        let surface = SurfaceTexture::new(size.width, size.height, &window);
        let mut pixels = Pixels::new(160, 144, surface).expect("Pixels error");

        let mut imgui = ImguiContext::create();
        imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE | ConfigFlags::VIEWPORTS_ENABLE;
        let mut platform = WinitPlatform::init(&mut imgui);
        platform.attach_window(imgui.io_mut(), &window, HiDpiMode::Rounded);
        let renderer_config = RendererConfig {
            texture_format: pixels.render_texture_format(),
            ..Default::default()
        };
        let mut renderer =
            Renderer::new(&mut imgui, pixels.device(), pixels.queue(), renderer_config);
        let mut state = 0xFFu8;
        let mut cursor_pos = PhysicalPosition::new(0.0, 0.0);

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Poll;
            platform.handle_event(imgui.io_mut(), &window, &event);
            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                    WindowEvent::Resized(size) => {
                        let _ = pixels.resize_surface(size.width, size.height);
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        cursor_pos = position;
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Right,
                        ..
                    } => {
                        if !ui_state.paused && cursor_in_screen(&window, cursor_pos) {
                            ui_state.paused = true;
                            ui_state.show_context = true;
                            ui_state.ctx_pos = [cursor_pos.x as f32, cursor_pos.y as f32];
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } => {
                        if ui_state.paused {
                            ui_state.paused = false;
                        }
                    }
                    WindowEvent::KeyboardInput { input, .. } => {
                        if !ui_state.paused {
                            if let Some(key) = input.virtual_keycode {
                                let pressed = input.state == ElementState::Pressed;
                                let mask = match key {
                                    VirtualKeyCode::Right => Some(0x01),
                                    VirtualKeyCode::Left => Some(0x02),
                                    VirtualKeyCode::Up => Some(0x04),
                                    VirtualKeyCode::Down => Some(0x08),
                                    VirtualKeyCode::S => Some(0x10),
                                    VirtualKeyCode::A => Some(0x20),
                                    VirtualKeyCode::LShift | VirtualKeyCode::RShift => Some(0x40),
                                    VirtualKeyCode::Return => Some(0x80),
                                    VirtualKeyCode::Escape => {
                                        if pressed {
                                            *control_flow = ControlFlow::Exit;
                                        }
                                        None
                                    }
                                    _ => None,
                                };
                                if let Some(mask) = mask {
                                    if pressed {
                                        state &= !mask;
                                    } else {
                                        state |= mask;
                                    }
                                    gb.mmu.input.update_state(state, &mut gb.mmu.if_reg);
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Event::MainEventsCleared => {
                    while !gb.mmu.ppu.frame_ready() && !ui_state.paused {
                        gb.cpu.step(&mut gb.mmu);
                    }

                    if gb.mmu.ppu.frame_ready() {
                        frame.copy_from_slice(gb.mmu.ppu.framebuffer());
                        gb.mmu.ppu.clear_frame_flag();
                    }

                    // UI built during RedrawRequested
                    window.request_redraw();

                    if args.debug && frame_count % 60 == 0 {
                        let serial = gb.mmu.take_serial();
                        if !serial.is_empty() {
                            print!("[SERIAL] ");
                            for b in &serial {
                                if b.is_ascii_graphic() || *b == b' ' {
                                    print!("{}", *b as char);
                                } else {
                                    print!("\\x{:02X}", b);
                                }
                            }
                            println!();
                        }

                        println!("{}", gb.cpu.debug_state());
                    }

                    frame_count += 1;
                }
                Event::RedrawRequested(_) => {
                    platform
                        .prepare_frame(imgui.io_mut(), &window)
                        .expect("prepare frame");
                    let ui = imgui.frame();
                    build_ui(&mut ui_state, ui);
                    platform.prepare_render(ui, &window);

                    let pixel_frame: &mut [u32] = bytemuck::cast_slice_mut(pixels.frame_mut());
                    for (dst, src) in pixel_frame.iter_mut().zip(&frame) {
                        *dst = 0xFF00_0000 | *src;
                    }
                    let draw_data = imgui.render();
                    let render_result = pixels.render_with(|encoder, render_target, context| {
                        context.scaling_renderer.render(encoder, render_target);

                        if draw_data.total_vtx_count > 0 {
                            let mut rpass =
                                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("imgui_pass"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: render_target,
                                        resolve_target: None,
                                        ops: wgpu::Operations {
                                            load: wgpu::LoadOp::Load,
                                            store: true,
                                        },
                                    })],
                                    depth_stencil_attachment: None,
                                });
                            renderer
                                .render(draw_data, pixels.queue(), pixels.device(), &mut rpass)
                                .expect("imgui render failed");
                        }
                        Ok(())
                    });
                    if render_result.is_err() {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                _ => {}
            }
        });
    } else {
        let frame_limit = args.frames;
        let cycle_limit = args.cycles;
        let second_limit = args.seconds.map(Duration::from_secs);

        let start = std::time::Instant::now();
        'headless: loop {
            while !gb.mmu.ppu.frame_ready() {
                gb.cpu.step(&mut gb.mmu);
                if let Some(max) = cycle_limit {
                    if gb.cpu.cycles >= max {
                        break 'headless;
                    }
                }
                if let Some(limit) = second_limit {
                    if start.elapsed() >= limit {
                        break 'headless;
                    }
                }
            }

            frame.copy_from_slice(gb.mmu.ppu.framebuffer());
            gb.mmu.ppu.clear_frame_flag();

            if args.debug && frame_count % 60 == 0 {
                let serial = gb.mmu.take_serial();
                if !serial.is_empty() {
                    print!("[SERIAL] ");
                    for b in &serial {
                        if b.is_ascii_graphic() || *b == b' ' {
                            print!("{}", *b as char);
                        } else {
                            print!("\\x{:02X}", b);
                        }
                    }
                    println!();
                }

                println!("{}", gb.cpu.debug_state());
            }

            frame_count += 1;

            if let Some(max) = frame_limit {
                if frame_count >= max as u64 {
                    break;
                }
            }
            if let Some(limit) = second_limit {
                if start.elapsed() >= limit {
                    break;
                }
            }
        }
    }

    gb.mmu.save_cart_ram();
}
