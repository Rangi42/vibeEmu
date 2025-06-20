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
use log::info;
use pixels::{Pixels, SurfaceTexture};
use std::sync::Arc;
use std::time::Duration;
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

fn handle_ui_event(event: &Event<()>, ui: &mut UiState) {
    if let Event::WindowEvent { event, .. } = event {
        if matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            }
        ) {
            ui.show_context = true;
        }
    }
}

fn build_ui(_ui: &mut UiState) {
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
        Some(apu::Apu::start_stream(Arc::clone(&gb.mmu.apu)))
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

        let _device = pixels.device();
        let _queue = pixels.queue();

        let mut state = 0xFFu8;

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Poll;
            handle_ui_event(&event, &mut ui_state);
            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                    WindowEvent::Resized(size) => {
                        let _ = pixels.resize_surface(size.width, size.height);
                    }
                    WindowEvent::KeyboardInput { input, .. } => {
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
                    _ => {}
                },
                Event::MainEventsCleared => {
                    while !gb.mmu.ppu.frame_ready() && !ui_state.paused {
                        gb.cpu.step(&mut gb.mmu);
                    }

                    build_ui(&mut ui_state);

                    frame.copy_from_slice(gb.mmu.ppu.framebuffer());
                    gb.mmu.ppu.clear_frame_flag();
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
                    pixels
                        .frame_mut()
                        .copy_from_slice(bytemuck::cast_slice(&frame));
                    if pixels.render().is_err() {
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
