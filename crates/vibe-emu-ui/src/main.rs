#![allow(dead_code)]

mod audio;
mod ui;

use clap::{Parser, ValueEnum};
use cpal::traits::StreamTrait;
use imgui::{ConfigFlags, Context as ImguiContext};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use log::{debug, error, info, warn};
use pixels::{Pixels, SurfaceTexture};
use rfd::FileDialog;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, Once, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use vibe_emu_core::serial::{LinkPort, NullLinkPort};
use vibe_emu_core::{cartridge::Cartridge, gameboy::GameBoy, hardware::CgbRevision};
use vibe_emu_mobile::{
    MobileAdapter, MobileAdapterDevice, MobileAddr, MobileConfig, MobileHost, MobileLinkPort,
    MobileNumber, MobileSockType, StdMobileHost,
};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Icon, Window, WindowAttributes};

use crossbeam_channel as cb;
use ui::snapshot::UiSnapshot;

fn load_window_icon() -> Option<Icon> {
    let icon_data = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../gfx/vibeEmu_512px.png"
    ));
    let cursor = Cursor::new(&icon_data[..]);
    let mut decoder = png::Decoder::new(cursor);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let buffer_size = reader.output_buffer_size()?;
    let mut buf = vec![0; buffer_size];
    let info = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..info.buffer_size()];
    let pixel_count = info.width as usize * info.height as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match reader.info().color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for chunk in data.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xFF]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in data {
                rgba.extend_from_slice(&[g, g, g, 0xFF]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in data.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        _ => return None,
    }
    Icon::from_rgba(rgba, info.width, info.height).ok()
}

const SCALE: u32 = 2;
const GB_FPS: f64 = 59.7275;
const FRAME_TIME: Duration = Duration::from_nanos((1e9_f64 / GB_FPS) as u64);
const FF_MULT: f32 = 4.0;
const AUDIO_WARMUP_TARGET_RATIO: f32 = 0.9;
const AUDIO_WARMUP_CHECK_INTERVAL: u32 = 1024;
const AUDIO_WARMUP_TIMEOUT_MS: u64 = 200;

#[derive(Default)]
struct UiState {
    paused: bool,
    show_context: bool,
    ctx_pos: [f32; 2],
    spawn_debugger: bool,
    spawn_vram: bool,
    pending_action: Option<UiAction>,
    serial_peripheral: SerialPeripheral,
    pending_serial_peripheral: Option<SerialPeripheral>,
}

enum UiAction {
    Reset,
    Load(Cartridge),
}

#[derive(Clone, Copy)]
struct Speed {
    factor: f32,
    fast: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum SerialPeripheral {
    #[default]
    None,
    MobileAdapter,
}

#[derive(Clone, Copy)]
enum EmuCommand {
    SetPaused(bool),
    SetSpeed(Speed),
    UpdateInput(u8),
    SetSerialPeripheral(SerialPeripheral),
    Shutdown,
}

enum UiToEmu {
    Command(EmuCommand),
    Action(UiAction),
}

#[derive(Clone, Copy, Debug)]
enum UserEvent {
    EmuWake,
}

struct EmuThreadChannels {
    rx: mpsc::Receiver<UiToEmu>,
    frame_tx: cb::Sender<EmuEvent>,
    serial_tx: cb::Sender<EmuEvent>,
    frame_pool_tx: cb::Sender<Vec<u32>>,
    frame_pool_rx: cb::Receiver<Vec<u32>>,
    wake_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum MobileDeviceArg {
    Blue,
    Yellow,
    Green,
    Red,
}

impl From<MobileDeviceArg> for MobileAdapterDevice {
    fn from(value: MobileDeviceArg) -> Self {
        match value {
            MobileDeviceArg::Blue => MobileAdapterDevice::Blue,
            MobileDeviceArg::Yellow => MobileAdapterDevice::Yellow,
            MobileDeviceArg::Green => MobileAdapterDevice::Green,
            MobileDeviceArg::Red => MobileAdapterDevice::Red,
        }
    }
}

enum EmuEvent {
    Frame { frame: Vec<u32>, frame_index: u64 },
    Serial { data: Vec<u8>, frame_index: u64 },
}

use ui::window::{UiWindow, WindowKind};

#[derive(Parser)]
struct Args {
    /// Path to ROM file
    rom: Option<std::path::PathBuf>,

    /// Force DMG mode
    #[arg(long, conflicts_with = "cgb")]
    dmg: bool,

    /// Use a neutral (non-green) DMG palette
    #[arg(long)]
    dmg_neutral: bool,

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

    /// Enable Mobile Adapter GB emulation via libmobile
    #[arg(long)]
    mobile: bool,

    /// Path to the persisted MOBILE_CONFIG_SIZE blob (defaults next to ROM)
    #[arg(long)]
    mobile_config: Option<std::path::PathBuf>,

    /// Adapter model to emulate
    #[arg(long, value_enum, default_value_t = MobileDeviceArg::Blue)]
    mobile_device: MobileDeviceArg,

    /// Mark the connection as unmetered (used by some games)
    #[arg(long)]
    mobile_unmetered: bool,

    /// Override DNS server 1 as ip:port (e.g. 8.8.8.8:53 or [2001:4860:4860::8888]:53)
    #[arg(long)]
    mobile_dns1: Option<String>,

    /// Override DNS server 2 as ip:port
    #[arg(long)]
    mobile_dns2: Option<String>,

    /// Override relay server as ip:port
    #[arg(long)]
    mobile_relay: Option<String>,

    /// Override P2P port (defaults to libmobile's default)
    #[arg(long)]
    mobile_p2p_port: Option<u16>,

    /// Emit Mobile Adapter diagnostics (raw serial bytes + libmobile debug + socket events)
    #[arg(long)]
    mobile_diag: bool,
}

struct DiagMobileHost {
    inner: Box<dyn MobileHost>,
}

impl DiagMobileHost {
    fn new(inner: Box<dyn MobileHost>) -> Self {
        Self { inner }
    }
}

impl MobileHost for DiagMobileHost {
    fn debug_log(&mut self, line: &str) {
        info!("[MOBILE] {line}");
        self.inner.debug_log(line);
    }

    fn update_number(&mut self, which: MobileNumber, number: Option<&str>) {
        info!("[MOBILE] update_number {:?} -> {:?}", which, number);
        self.inner.update_number(which, number);
    }

    fn config_read(&mut self, dest: &mut [u8], offset: usize) -> bool {
        self.inner.config_read(dest, offset)
    }

    fn config_write(&mut self, src: &[u8], offset: usize) -> bool {
        self.inner.config_write(src, offset)
    }

    fn sock_open(
        &mut self,
        conn: u32,
        socktype: MobileSockType,
        addr: &MobileAddr,
        bind_port: u16,
    ) -> bool {
        let ok = self.inner.sock_open(conn, socktype, addr, bind_port);
        info!(
            "[MOBILE] sock_open conn={} type={:?} addr={:?} bind_port={} -> {}",
            conn, socktype, addr, bind_port, ok
        );
        ok
    }

    fn sock_close(&mut self, conn: u32) {
        info!("[MOBILE] sock_close conn={conn}");
        self.inner.sock_close(conn);
    }

    fn sock_connect(&mut self, conn: u32, addr: &MobileAddr) -> i32 {
        let rc = self.inner.sock_connect(conn, addr);
        info!(
            "[MOBILE] sock_connect conn={} addr={:?} -> {}",
            conn, addr, rc
        );
        rc
    }

    fn sock_listen(&mut self, conn: u32) -> bool {
        let ok = self.inner.sock_listen(conn);
        info!("[MOBILE] sock_listen conn={} -> {}", conn, ok);
        ok
    }

    fn sock_accept(&mut self, conn: u32) -> bool {
        let ok = self.inner.sock_accept(conn);
        info!("[MOBILE] sock_accept conn={} -> {}", conn, ok);
        ok
    }

    fn sock_send(&mut self, conn: u32, data: &[u8], addr: Option<&MobileAddr>) -> i32 {
        let rc = self.inner.sock_send(conn, data, addr);
        info!(
            "[MOBILE] sock_send conn={} len={} addr={:?} -> {}",
            conn,
            data.len(),
            addr,
            rc
        );
        rc
    }

    fn sock_recv(
        &mut self,
        conn: u32,
        mut data: Option<&mut [u8]>,
        mut addr_out: Option<&mut MobileAddr>,
    ) -> i32 {
        let rc = self
            .inner
            .sock_recv(conn, data.as_deref_mut(), addr_out.as_deref_mut());

        if rc > 0 {
            let n = rc as usize;
            let preview_len = n.min(32);
            match (data.as_deref(), addr_out.as_deref()) {
                (Some(buf), Some(addr)) => {
                    info!(
                        "[MOBILE] sock_recv conn={} -> {} bytes from {:?} (first {:02X?}{})",
                        conn,
                        rc,
                        addr,
                        &buf[..preview_len],
                        if n > preview_len { "…" } else { "" }
                    );
                }
                (Some(buf), None) => {
                    info!(
                        "[MOBILE] sock_recv conn={} -> {} bytes (first {:02X?}{})",
                        conn,
                        rc,
                        &buf[..preview_len],
                        if n > preview_len { "…" } else { "" }
                    );
                }
                _ => {
                    info!("[MOBILE] sock_recv conn={} -> {} bytes", conn, rc);
                }
            }
        } else {
            info!("[MOBILE] sock_recv conn={} -> {}", conn, rc);
        }

        rc
    }
}

struct DiagMobileLinkPort {
    adapter: Arc<Mutex<MobileAdapter>>,
}

impl DiagMobileLinkPort {
    fn new(adapter: Arc<Mutex<MobileAdapter>>) -> Self {
        Self { adapter }
    }
}

impl LinkPort for DiagMobileLinkPort {
    fn transfer(&mut self, byte: u8) -> u8 {
        let mut adapter = match self.adapter.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("mobile adapter mutex poisoned; recovering");
                poisoned.into_inner()
            }
        };
        let rx = adapter.transfer_byte(byte).unwrap_or(0xFF);
        debug!("[MOBILE][SIO] tx={:02X} rx={:02X}", byte, rx);
        rx
    }
}

fn parse_mobile_addr(s: &str) -> Result<MobileAddr, String> {
    let sock: std::net::SocketAddr = s
        .parse()
        .map_err(|e| format!("invalid socket address '{s}': {e}"))?;

    Ok(match sock {
        std::net::SocketAddr::V4(v4) => MobileAddr::V4 {
            host: v4.ip().octets(),
            port: v4.port(),
        },
        std::net::SocketAddr::V6(v6) => MobileAddr::V6 {
            host: v6.ip().octets(),
            port: v6.port(),
        },
    })
}

fn game_viewport_rect(
    window: &winit::window::Window,
    fb_w: u32,
    fb_h: u32,
) -> Option<(f64, f64, f64, f64)> {
    let size = window.inner_size();
    if size.width == 0 || size.height == 0 || fb_w == 0 || fb_h == 0 {
        return None;
    }

    // Approximate Pixels' scaling behavior: preserve aspect ratio, integer scale,
    // centered with letterboxing.
    let scale_x = size.width / fb_w;
    let scale_y = size.height / fb_h;
    let scale = scale_x.min(scale_y);
    if scale == 0 {
        return None;
    }

    let vp_w = (fb_w * scale) as f64;
    let vp_h = (fb_h * scale) as f64;
    let x0 = ((size.width as f64) - vp_w) / 2.0;
    let y0 = ((size.height as f64) - vp_h) / 2.0;
    Some((x0, y0, x0 + vp_w, y0 + vp_h))
}

fn cursor_in_screen(window: &winit::window::Window, pos: PhysicalPosition<f64>) -> bool {
    let Some((x0, y0, x1, y1)) = game_viewport_rect(window, 160, 144) else {
        return false;
    };
    pos.x >= x0 && pos.x < x1 && pos.y >= y0 && pos.y < y1
}

fn sync_imgui_display_for_window(imgui: &mut ImguiContext, window: &winit::window::Window) {
    // imgui-winit-support updates these fields from *events* and stores them in the global Io.
    // With multiple OS windows sharing one ImGuiContext, resizing one window can therefore
    // affect rendering scale in another unless we re-sync before each frame.
    let scale_factor = window.scale_factor();
    let hidpi_factor = scale_factor.round();

    imgui.io_mut().display_framebuffer_scale = [hidpi_factor as f32, hidpi_factor as f32];

    // Match imgui-winit-support's `scale_size_from_winit` behavior for HiDpiMode::Rounded:
    // logical -> physical (window scale) -> logical (hidpi factor)
    let logical_size = window.inner_size().to_logical::<f64>(scale_factor);
    let logical_size = logical_size
        .to_physical::<f64>(scale_factor)
        .to_logical::<f64>(hidpi_factor);

    imgui.io_mut().display_size = [logical_size.width as f32, logical_size.height as f32];
}

#[cfg(target_os = "windows")]
fn enforce_square_corners(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};

    attrs.with_corner_preference(CornerPreference::DoNotRound)
}

#[cfg(not(target_os = "windows"))]
fn enforce_square_corners(attrs: WindowAttributes) -> WindowAttributes {
    attrs
}

fn spawn_debugger_window(
    event_loop: &ActiveEventLoop,
    platform: &mut WinitPlatform,
    imgui: &mut ImguiContext,
    windows: &mut HashMap<winit::window::WindowId, UiWindow>,
) {
    use winit::dpi::LogicalSize;
    let attrs = enforce_square_corners(
        Window::default_attributes()
            .with_title("vibeEmu \u{2013} Debugger")
            .with_window_icon(load_window_icon())
            .with_inner_size(LogicalSize::new((160 * SCALE) as f64, (144 * SCALE) as f64)),
    );
    let w = match event_loop.create_window(attrs) {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to create debugger window: {e}");
            return;
        }
    };

    let size = w.inner_size();
    let surface = pixels::SurfaceTexture::new(size.width, size.height, &w);
    let pixels = match pixels::Pixels::new(1, 1, surface) {
        Ok(p) => p,
        Err(e) => {
            error!("Pixels init failed (debugger window): {e}");
            return;
        }
    };

    platform.attach_window(imgui.io_mut(), &w, HiDpiMode::Rounded);

    let ui_win = UiWindow::new(WindowKind::Debugger, w, pixels, (1, 1), imgui);
    let id = ui_win.win.id();
    windows.insert(id, ui_win);
    if let Some(win) = windows.get_mut(&id) {
        win.resize(win.win.inner_size());
    }
}

fn spawn_vram_window(
    event_loop: &ActiveEventLoop,
    platform: &mut WinitPlatform,
    imgui: &mut ImguiContext,
    windows: &mut HashMap<winit::window::WindowId, UiWindow>,
) {
    use winit::dpi::LogicalSize;
    let attrs = enforce_square_corners(
        Window::default_attributes()
            .with_title("vibeEmu \u{2013} VRAM")
            .with_window_icon(load_window_icon())
            .with_inner_size(LogicalSize::new(640.0, 600.0)),
    );
    let w = match event_loop.create_window(attrs) {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to create VRAM window: {e}");
            return;
        }
    };

    let size = w.inner_size();
    let surface = pixels::SurfaceTexture::new(size.width, size.height, &w);
    let pixels = match pixels::Pixels::new(1, 1, surface) {
        Ok(p) => p,
        Err(e) => {
            error!("Pixels init failed (VRAM window): {e}");
            return;
        }
    };

    platform.attach_window(imgui.io_mut(), &w, HiDpiMode::Rounded);

    let ui_win = UiWindow::new(WindowKind::VramViewer, w, pixels, (1, 1), imgui);
    let id = ui_win.win.id();
    windows.insert(id, ui_win);
    if let Some(win) = windows.get_mut(&id) {
        win.resize(win.win.inner_size());
    }
}

#[allow(clippy::too_many_arguments)]
fn run_emulator_thread(
    gb: Arc<Mutex<GameBoy>>,
    ui_snapshot: Arc<RwLock<UiSnapshot>>,
    mut speed: Speed,
    debug: bool,
    mobile: Option<Arc<Mutex<MobileAdapter>>>,
    mut serial_peripheral: SerialPeripheral,
    mobile_diag: bool,
    channels: EmuThreadChannels,
) {
    let EmuThreadChannels {
        rx,
        frame_tx,
        serial_tx,
        frame_pool_tx,
        frame_pool_rx,
        wake_proxy,
    } = channels;

    fn apply_serial_peripheral(
        gb: &mut GameBoy,
        mobile: &Option<Arc<Mutex<MobileAdapter>>>,
        desired: SerialPeripheral,
        mobile_diag: bool,
        serial_peripheral: &mut SerialPeripheral,
        mobile_active: &mut bool,
        mobile_time_accum_ns: &mut u128,
    ) {
        match desired {
            SerialPeripheral::None => {
                if let Some(mobile) = mobile.as_ref()
                    && let Ok(mut adapter) = mobile.lock()
                {
                    let _ = adapter.stop();
                }
                gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
                *mobile_active = false;
                *serial_peripheral = SerialPeripheral::None;
            }
            SerialPeripheral::MobileAdapter => {
                let Some(mobile) = mobile.as_ref() else {
                    warn!("Mobile Adapter requested but unavailable");
                    gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
                    *mobile_active = false;
                    *serial_peripheral = SerialPeripheral::None;
                    *mobile_time_accum_ns = 0;
                    return;
                };

                if let Ok(mut adapter) = mobile.lock() {
                    let _ = adapter.stop();
                    if let Err(e) = adapter.start() {
                        warn!("Failed to start Mobile Adapter: {e}");
                        gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
                        *mobile_active = false;
                        *serial_peripheral = SerialPeripheral::None;
                        *mobile_time_accum_ns = 0;
                        return;
                    }
                }

                if mobile_diag {
                    gb.mmu
                        .serial
                        .connect(Box::new(DiagMobileLinkPort::new(Arc::clone(mobile))));
                } else {
                    gb.mmu
                        .serial
                        .connect(Box::new(MobileLinkPort::new(Arc::clone(mobile))));
                }

                *mobile_active = true;
                *serial_peripheral = SerialPeripheral::MobileAdapter;
            }
        }

        *mobile_time_accum_ns = 0;
    }

    let mut paused = false;
    let mut frame_count = 0u64;
    let mut next_frame = Instant::now() + FRAME_TIME;
    let mut audio_stream = None;
    let mut mobile_time_accum_ns: u128 = 0;
    let mut mobile_active = serial_peripheral == SerialPeripheral::MobileAdapter;

    if let Ok(mut gb) = gb.lock() {
        apply_serial_peripheral(
            &mut gb,
            &mobile,
            serial_peripheral,
            mobile_diag,
            &mut serial_peripheral,
            &mut mobile_active,
            &mut mobile_time_accum_ns,
        );
        rebuild_audio_stream(&mut gb, speed, &mut audio_stream);
    }

    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                UiToEmu::Command(cmd) => match cmd {
                    EmuCommand::SetPaused(p) => {
                        paused = p;
                        next_frame = Instant::now() + FRAME_TIME;
                    }
                    EmuCommand::SetSpeed(new_speed) => {
                        speed = new_speed;
                        next_frame = Instant::now() + FRAME_TIME;
                        if let Ok(mut gb) = gb.lock() {
                            gb.mmu.apu.set_speed(speed.factor);
                        }
                    }
                    EmuCommand::UpdateInput(state) => {
                        if let Ok(mut gb) = gb.lock() {
                            let mmu = &mut gb.mmu;
                            let if_reg = &mut mmu.if_reg;
                            mmu.input.update_state(state, if_reg);
                        }
                    }
                    EmuCommand::SetSerialPeripheral(peripheral) => {
                        if let Ok(mut gb) = gb.lock() {
                            apply_serial_peripheral(
                                &mut gb,
                                &mobile,
                                peripheral,
                                mobile_diag,
                                &mut serial_peripheral,
                                &mut mobile_active,
                                &mut mobile_time_accum_ns,
                            );
                        }
                    }
                    EmuCommand::Shutdown => {
                        if let Ok(mut gb) = gb.lock() {
                            gb.mmu.save_cart_ram();
                        }
                        if let Some(mobile) = mobile.as_ref()
                            && let Ok(mut adapter) = mobile.lock()
                        {
                            let _ = adapter.stop();
                        }
                        return;
                    }
                },
                UiToEmu::Action(action) => {
                    if let Ok(mut gb) = gb.lock() {
                        apply_ui_action(action, &mut gb, &mut audio_stream, speed);
                        // GameBoy::reset rebuilds the MMU (including Serial), so restore the
                        // currently selected serial peripheral after any reset/load.
                        apply_serial_peripheral(
                            &mut gb,
                            &mobile,
                            serial_peripheral,
                            mobile_diag,
                            &mut serial_peripheral,
                            &mut mobile_active,
                            &mut mobile_time_accum_ns,
                        );
                        gb.mmu.ppu.clear_frame_flag();
                        frame_count = 0;
                        next_frame = Instant::now() + FRAME_TIME;
                    }
                }
            }
        }

        if paused {
            thread::sleep(Duration::from_millis(1));
            continue;
        }

        let frame_start = Instant::now();
        let mut frame_buf: Option<Vec<u32>> = None;
        let mut serial = None;

        if let Ok(mut gb) = gb.lock() {
            gb.mmu.apu.set_speed(speed.factor);

            {
                let (cpu, mmu) = {
                    let GameBoy { cpu, mmu, .. } = &mut *gb;
                    (cpu, mmu)
                };
                while !mmu.ppu.frame_ready() {
                    cpu.step(mmu);
                }

                // Avoid allocating every frame. If no free buffers are
                // available, drop this frame rather than allocating.
                if let Ok(mut buf) = frame_pool_rx.try_recv() {
                    buf.as_mut_slice().copy_from_slice(mmu.ppu.framebuffer());
                    frame_buf = Some(buf);
                }
                mmu.ppu.clear_frame_flag();
            }

            // Publish a UI snapshot while we already hold the emulation lock.
            // Use try_write to avoid stalling emulation if the UI is mid-draw.
            if let Ok(mut snap) = ui_snapshot.try_write() {
                *snap = UiSnapshot::from_gb(&mut gb);
            }

            if !speed.fast {
                let elapsed = frame_start.elapsed();
                let warn_threshold = FRAME_TIME + FRAME_TIME / 2;
                if elapsed > warn_threshold {
                    warn!(
                        "Frame emulation exceeded budget: {:?} vs {:?} (audio queue {} / {})",
                        elapsed,
                        FRAME_TIME,
                        gb.mmu.apu.queued_frames(),
                        gb.mmu.apu.max_queue_capacity()
                    );
                }
            }

            if debug && frame_count.is_multiple_of(60) {
                let out = gb.mmu.take_serial();
                if !out.is_empty() {
                    serial = Some(out);
                }
                println!("{}", gb.cpu.debug_state());
            }
        }

        if let Some(frame) = frame_buf {
            // If the UI is behind, drop frames instead of queueing unbounded.
            let evt = EmuEvent::Frame {
                frame,
                frame_index: frame_count,
            };

            match frame_tx.send_timeout(evt, Duration::ZERO) {
                Ok(()) => {
                    let _ = wake_proxy.send_event(UserEvent::EmuWake);
                }
                Err(
                    cb::SendTimeoutError::Timeout(evt) | cb::SendTimeoutError::Disconnected(evt),
                ) => {
                    // Return buffer to the pool if we couldn't send.
                    if let EmuEvent::Frame { frame, .. } = evt {
                        let _ = frame_pool_tx.send_timeout(frame, Duration::ZERO);
                    }
                }
            }
        }

        if let Some(serial) = serial {
            let _ = serial_tx.send_timeout(
                EmuEvent::Serial {
                    data: serial,
                    frame_index: frame_count,
                },
                Duration::ZERO,
            );
            let _ = wake_proxy.send_event(UserEvent::EmuWake);
        }

        frame_count = frame_count.wrapping_add(1);

        if mobile_active && let Some(mobile) = mobile.as_ref() {
            // Advance emulated time by one frame and drive libmobile.
            mobile_time_accum_ns += FRAME_TIME.as_nanos();
            let delta_ms = (mobile_time_accum_ns / 1_000_000) as u32;
            mobile_time_accum_ns %= 1_000_000;

            if delta_ms != 0
                && let Ok(mut adapter) = mobile.lock()
            {
                let _ = adapter.poll(delta_ms);
            }
        }

        if !speed.fast {
            let now = Instant::now();
            if now < next_frame {
                thread::sleep(next_frame - now);
            }
            next_frame += FRAME_TIME;
        } else {
            next_frame = Instant::now();
        }
    }
}

fn draw_debugger(pixels: &mut Pixels, snapshot: &UiSnapshot, ui: &imgui::Ui) {
    let _ = pixels.frame_mut();
    let cpu = &snapshot.cpu;

    let display = ui.io().display_size;
    let flags = imgui::WindowFlags::NO_MOVE
        | imgui::WindowFlags::NO_RESIZE
        | imgui::WindowFlags::NO_COLLAPSE;

    ui.window("Debugger")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size(display, imgui::Condition::Always)
        .flags(flags)
        .build(|| {
            if let Some(_table) = ui.begin_table("regs", 2) {
                ui.table_next_row();
                ui.table_next_column();
                ui.text("A");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.a));

                ui.table_next_column();
                ui.text("F");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.f));

                ui.table_next_column();
                ui.text("B");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.b));

                ui.table_next_column();
                ui.text("C");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.c));

                ui.table_next_column();
                ui.text("D");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.d));

                ui.table_next_column();
                ui.text("E");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.e));

                ui.table_next_column();
                ui.text("H");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.h));

                ui.table_next_column();
                ui.text("L");
                ui.table_next_column();
                ui.text(format!("{:02X}", cpu.l));

                ui.table_next_column();
                ui.text("SP");
                ui.table_next_column();
                ui.text(format!("{:04X}", cpu.sp));

                ui.table_next_column();
                ui.text("PC");
                ui.table_next_column();
                ui.text(format!("{:04X}", cpu.pc));

                ui.table_next_column();
                ui.text("IME");
                ui.table_next_column();
                ui.text(format!("{}", cpu.ime));

                ui.table_next_column();
                ui.text("Cycles");
                ui.table_next_column();
                ui.text(format!("{}", cpu.cycles));
            }
        });
}

fn draw_vram(win: &mut ui::window::UiWindow, snapshot: &UiSnapshot, ui: &imgui::Ui) {
    let _ = win.pixels.frame_mut();
    if let Some(viewer) = win.vram_viewer.as_mut() {
        viewer.ui(
            ui,
            &snapshot.ppu,
            &mut win.renderer,
            win.pixels.device(),
            win.pixels.queue(),
        );
    } else {
        ui.text("VRAM viewer not initialized");
    }
}

fn draw_game_screen(pixels: &mut Pixels, frame: &[u32]) {
    for (dst, &src) in pixels.frame_mut().chunks_exact_mut(4).zip(frame.iter()) {
        let r = ((src >> 16) & 0xFF) as u8;
        let g = ((src >> 8) & 0xFF) as u8;
        let b = (src & 0xFF) as u8;
        dst[0] = r;
        dst[1] = g;
        dst[2] = b;
        dst[3] = 0xFF;
    }
}

#[allow(clippy::too_many_arguments)]
fn build_ui(state: &mut UiState, ui: &imgui::Ui, mobile_available: bool) {
    if state.show_context {
        let flags = imgui::WindowFlags::NO_TITLE_BAR
            | imgui::WindowFlags::NO_MOVE
            | imgui::WindowFlags::NO_DECORATION;
        let mut open = state.show_context;
        let mut close_menu = false;
        ui.window("ctx")
            .position(state.ctx_pos, imgui::Condition::Always)
            .flags(flags)
            .always_auto_resize(true)
            .opened(&mut open)
            .build(|| {
                if ui.button("Load ROM") {
                    if let Some(path) = FileDialog::new()
                        .add_filter("Game Boy ROM", &["gb", "gbc"])
                        .pick_file()
                        && let Ok(cart) = Cartridge::from_file(&path)
                    {
                        state.pending_action = Some(UiAction::Load(cart));
                        state.paused = false;
                    }
                    close_menu = true;
                }
                if ui.button("Reset GB") {
                    state.pending_action = Some(UiAction::Reset);
                    state.paused = false;
                    close_menu = true;
                }

                ui.separator();
                ui.text("Serial Peripheral");

                if mobile_available {
                    let items = ["None", "Mobile Adapter GB"];
                    let mut selected = match state.serial_peripheral {
                        SerialPeripheral::None => 0,
                        SerialPeripheral::MobileAdapter => 1,
                    };
                    if ui.combo_simple_string("##serial_peripheral", &mut selected, &items) {
                        let new_value = match selected {
                            1 => SerialPeripheral::MobileAdapter,
                            _ => SerialPeripheral::None,
                        };
                        if new_value != state.serial_peripheral {
                            state.serial_peripheral = new_value;
                            state.pending_serial_peripheral = Some(new_value);
                        }
                    }
                } else {
                    state.serial_peripheral = SerialPeripheral::None;
                    ui.text_disabled("Mobile Adapter GB (unavailable)");
                }

                if ui.button("Debugger") {
                    state.spawn_debugger = true;
                    close_menu = true;
                }
                if ui.button("VRAM Viewer") {
                    state.spawn_vram = true;
                    close_menu = true;
                }
            });
        state.show_context = open && !close_menu;
    }
}

fn configure_wgpu_backend() {
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // Prefer DirectX on Windows to avoid buggy Vulkan/ANGLE present modes.
        unsafe {
            std::env::set_var("WGPU_BACKEND", "dx12");
        }
    }
}

fn log_wgpu_adapter_once(pixels: &Pixels) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let adapter_info = pixels.adapter().get_info();
        info!(
            "WGPU adapter: {} (type={:?}, backend={:?}, vendor=0x{:X}, device=0x{:X}); driver='{}' driver_info='{}'",
            adapter_info.name,
            adapter_info.device_type,
            adapter_info.backend,
            adapter_info.vendor,
            adapter_info.device,
            adapter_info.driver,
            adapter_info.driver_info
        );

        if let Some(forced) = std::env::var_os("WGPU_BACKEND") {
            info!("WGPU_BACKEND={}", forced.to_string_lossy());
        }
    });
}

fn prime_audio_queue(gb: &mut GameBoy) -> (usize, usize) {
    let capacity = gb.mmu.apu.max_queue_capacity().max(1);
    let target_frames = ((capacity as f32) * AUDIO_WARMUP_TARGET_RATIO).ceil() as usize;

    let mut queued = gb.mmu.apu.queued_frames();
    if queued >= target_frames {
        return (queued, target_frames);
    }

    let deadline = Instant::now() + Duration::from_millis(AUDIO_WARMUP_TIMEOUT_MS);
    let mut steps = 0u32;
    loop {
        gb.cpu.step(&mut gb.mmu);
        steps += 1;

        if steps >= AUDIO_WARMUP_CHECK_INTERVAL {
            steps = 0;
            let now = Instant::now();
            queued = gb.mmu.apu.queued_frames();
            if queued >= target_frames || now >= deadline {
                break;
            }
        }
    }

    if queued < target_frames {
        queued = gb.mmu.apu.queued_frames();
    }

    (queued, target_frames)
}

fn prime_and_play_audio(gb: &mut GameBoy, audio_stream: &mut Option<cpal::Stream>) {
    let primed = audio_stream.as_ref().map(|_| prime_audio_queue(gb));

    if let Some((queued, target)) = &primed {
        if *queued >= *target {
            info!("Primed audio queue with {queued} / {target} frames before starting playback");
        } else {
            warn!(
                "Audio warmup timed out after priming {queued} / {target} frames; startup may glitch"
            );
        }
    }

    if let Some(stream) = audio_stream.as_ref() {
        if let Err(e) = stream.play() {
            warn!("Failed to start audio stream: {e}");
            *audio_stream = None;
        } else if let Some((queued, target)) = &primed {
            info!("Audio playback started with {queued} primed frames (capacity {target})");
        } else {
            info!("Audio playback started");
        }
    }
}

fn rebuild_audio_stream(gb: &mut GameBoy, speed: Speed, audio_stream: &mut Option<cpal::Stream>) {
    if let Some(stream) = audio_stream.take() {
        drop(stream);
    }

    gb.mmu.apu.set_speed(speed.factor);

    *audio_stream = audio::start_stream(&mut gb.mmu.apu, false);
    if audio_stream.is_none() {
        warn!("Audio output disabled; continuing without sound");
        return;
    }

    prime_and_play_audio(gb, audio_stream);
}

fn apply_ui_action(
    action: UiAction,
    gb: &mut GameBoy,
    audio_stream: &mut Option<cpal::Stream>,
    speed: Speed,
) {
    match action {
        UiAction::Reset => {
            info!("Resetting Game Boy");
            gb.reset();
        }
        UiAction::Load(cart) => {
            info!("Loading new ROM");
            gb.reset();
            gb.mmu.load_cart(cart);
        }
    }

    rebuild_audio_stream(gb, speed, audio_stream);
}

fn main() {
    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    // Reduce noisy GPU backend logs (especially in release builds). Users can
    // override these via `RUST_LOG` if they explicitly want them.
    logger.filter_module("wgpu", log::LevelFilter::Warn);
    logger.filter_module("wgpu_core", log::LevelFilter::Warn);
    logger.filter_module("wgpu_hal", log::LevelFilter::Warn);
    logger.filter_module("naga", log::LevelFilter::Warn);
    logger.format_timestamp_millis().init();

    configure_wgpu_backend();

    let args = Args::parse();

    info!("Starting emulator");

    let rom_path = match args.rom {
        Some(p) => p,
        None => {
            error!("No ROM supplied");
            return;
        }
    };

    let cart = match Cartridge::from_file(&rom_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load ROM: {e}");
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
    let mut gb = GameBoy::new_with_revision(cgb_mode, CgbRevision::default());
    gb.mmu.load_cart(cart);
    // If user requested a neutral/non-green DMG palette, apply it.
    if !cgb_mode && args.dmg_neutral {
        const NEUTRAL_DMG_PALETTE: [u32; 4] = [0x00E0F8D0, 0x0088C070, 0x00346856, 0x00081820];
        gb.mmu.ppu.set_dmg_palette(NEUTRAL_DMG_PALETTE);
    }

    if let Some(path) = args.bootrom {
        match std::fs::read(&path) {
            Ok(data) => {
                gb.mmu.load_boot_rom(data);
                // Start executing from the boot ROM entry point.
                gb.cpu.pc = 0x0000;
            }
            Err(e) => warn!("Failed to load boot ROM: {e}"),
        }
    }

    info!(
        "Emulator initialized in {} mode",
        if cgb_mode { "CGB" } else { "DMG" }
    );

    let headless = args.headless;
    let debug_enabled = args.debug;
    let frame_limit = args.frames;
    let cycle_limit = args.cycles;
    let second_limit = args.seconds.map(Duration::from_secs);

    let mut frame = vec![0u32; 160 * 144];

    let mut serial_peripheral = if args.mobile {
        SerialPeripheral::MobileAdapter
    } else {
        SerialPeripheral::None
    };

    let config_path = args
        .mobile_config
        .clone()
        .unwrap_or_else(|| rom_path.with_extension("mobile"));

    let mobile = {
        let base_host: Box<dyn MobileHost> = Box::new(StdMobileHost::new(config_path));
        let host: Box<dyn MobileHost> = if args.mobile_diag {
            Box::new(DiagMobileHost::new(base_host))
        } else {
            base_host
        };

        match MobileAdapter::new(host) {
            Ok(mut adapter) => {
                let mut cfg = MobileConfig {
                    device: args.mobile_device.into(),
                    unmetered: args.mobile_unmetered,
                    p2p_port: args.mobile_p2p_port,
                    ..Default::default()
                };

                if let Some(s) = args.mobile_dns1.as_deref() {
                    match parse_mobile_addr(s) {
                        Ok(addr) => cfg.dns1 = addr,
                        Err(e) => warn!("Ignoring --mobile-dns1: {e}"),
                    }
                }
                if let Some(s) = args.mobile_dns2.as_deref() {
                    match parse_mobile_addr(s) {
                        Ok(addr) => cfg.dns2 = addr,
                        Err(e) => warn!("Ignoring --mobile-dns2: {e}"),
                    }
                }
                if let Some(s) = args.mobile_relay.as_deref() {
                    match parse_mobile_addr(s) {
                        Ok(addr) => cfg.relay = addr,
                        Err(e) => warn!("Ignoring --mobile-relay: {e}"),
                    }
                }

                let _ = adapter.apply_config(&cfg);
                Some(Arc::new(Mutex::new(adapter)))
            }
            Err(e) => {
                warn!("Mobile Adapter backend unavailable: {e}");
                None
            }
        }
    };

    match serial_peripheral {
        SerialPeripheral::None => {
            gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
        }
        SerialPeripheral::MobileAdapter => {
            if let Some(mobile) = mobile.as_ref() {
                if let Ok(mut adapter) = mobile.lock()
                    && let Err(e) = adapter.start()
                {
                    warn!("Failed to start Mobile Adapter: {e}");
                    gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
                    serial_peripheral = SerialPeripheral::None;
                }

                if serial_peripheral == SerialPeripheral::MobileAdapter {
                    if args.mobile_diag {
                        gb.mmu
                            .serial
                            .connect(Box::new(DiagMobileLinkPort::new(Arc::clone(mobile))));
                    } else {
                        gb.mmu
                            .serial
                            .connect(Box::new(MobileLinkPort::new(Arc::clone(mobile))));
                    }
                }
            } else {
                warn!("Mobile Adapter selected but backend not available");
                gb.mmu.serial.connect(Box::new(NullLinkPort::default()));
                serial_peripheral = SerialPeripheral::None;
            }
        }
    }

    if !headless {
        let gb = Arc::new(Mutex::new(gb));
        let ui_snapshot = Arc::new(RwLock::new(UiSnapshot::default()));
        let mut speed = Speed {
            factor: 1.0,
            fast: false,
        };
        let mut ui_state = UiState {
            serial_peripheral,
            ..Default::default()
        };

        let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
            Ok(el) => el,
            Err(e) => {
                error!("Failed to initialize winit event loop: {e}");
                return;
            }
        };
        let wake_proxy = event_loop.create_proxy();

        let (to_emu_tx, to_emu_rx) = mpsc::channel();
        // Bounded channels so UI stalls can't grow memory without bound.
        let (from_emu_frame_tx, from_emu_frame_rx) = cb::bounded(1);
        let (from_emu_serial_tx, from_emu_serial_rx) = cb::bounded(64);
        // Buffer pool to avoid per-frame allocations in the emulator thread.
        let (frame_pool_tx, frame_pool_rx) = cb::bounded::<Vec<u32>>(2);
        for _ in 0..2 {
            let _ = frame_pool_tx.send(vec![0u32; 160 * 144]);
        }
        let emu_gb = Arc::clone(&gb);
        let emu_snapshot = Arc::clone(&ui_snapshot);
        let emu_mobile = mobile.clone();
        let emu_serial_peripheral = serial_peripheral;
        let emu_mobile_diag = args.mobile_diag;
        let emu_frame_pool_tx = frame_pool_tx.clone();
        let emu_handle = thread::spawn(move || {
            run_emulator_thread(
                emu_gb,
                emu_snapshot,
                speed,
                debug_enabled,
                emu_mobile,
                emu_serial_peripheral,
                emu_mobile_diag,
                EmuThreadChannels {
                    rx: to_emu_rx,
                    frame_tx: from_emu_frame_tx,
                    serial_tx: from_emu_serial_tx,
                    frame_pool_tx: emu_frame_pool_tx,
                    frame_pool_rx,
                    wake_proxy,
                },
            );
        });
        let mut emu_handle = Some(emu_handle);
        let mut sent_shutdown = false;

        let _ = to_emu_tx.send(UiToEmu::Command(EmuCommand::UpdateInput(0xFF)));
        let attrs = enforce_square_corners(
            Window::default_attributes()
                .with_title("vibeEmu")
                .with_window_icon(load_window_icon())
                .with_inner_size(winit::dpi::LogicalSize::new(
                    (160 * SCALE) as f64,
                    (144 * SCALE) as f64,
                )),
        );
        #[allow(deprecated)]
        let window = match event_loop.create_window(attrs) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create main window: {e}");
                return;
            }
        };

        let size = window.inner_size();
        let surface = SurfaceTexture::new(size.width, size.height, &window);
        let pixels = match Pixels::new(160, 144, surface) {
            Ok(p) => p,
            Err(e) => {
                error!("Pixels init failed (main window): {e}");
                return;
            }
        };

        log_wgpu_adapter_once(&pixels);

        let mut imgui = ImguiContext::create();
        imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;
        let mut platform = WinitPlatform::new(&mut imgui);
        platform.attach_window(imgui.io_mut(), &window, HiDpiMode::Rounded);

        let mut windows = HashMap::new();
        let main_win = UiWindow::new(WindowKind::Main, window, pixels, (160, 144), &mut imgui);
        let main_id = main_win.win.id();
        windows.insert(main_id, main_win);
        if let Some(win) = windows.get_mut(&main_id) {
            win.resize(win.win.inner_size());
        }

        let mut state = 0xFFu8;
        let mut cursor_pos = PhysicalPosition::new(0.0, 0.0);

        #[allow(deprecated)]
        let _ = event_loop.run(move |event, target| {
            target.set_control_flow(if ui_state.paused {
                ControlFlow::Wait
            } else {
                ControlFlow::WaitUntil(Instant::now() + FRAME_TIME)
            });
            match &event {
                Event::UserEvent(UserEvent::EmuWake) => {
                    let mut got_frame = false;

                    while let Ok(evt) = from_emu_frame_rx.try_recv() {
                        if let EmuEvent::Frame {
                            frame: mut incoming,
                            frame_index: _,
                        } = evt
                            && incoming.len() == frame.len()
                        {
                            std::mem::swap(&mut frame, &mut incoming);
                            let _ = frame_pool_tx.send_timeout(incoming, Duration::ZERO);
                            got_frame = true;
                        }
                    }

                    while let Ok(evt) = from_emu_serial_rx.try_recv() {
                        if let EmuEvent::Serial { data, frame_index } = evt
                            && !data.is_empty()
                        {
                            print!("[SERIAL {frame_index}] ");
                            for b in &data {
                                if b.is_ascii_graphic() || *b == b' ' {
                                    print!("{}", *b as char);
                                } else {
                                    print!("\\x{b:02X}");
                                }
                            }
                            println!();
                        }
                    }

                    if got_frame {
                        for win in windows.values() {
                            win.win.request_redraw();
                        }
                    }
                }
                Event::WindowEvent {
                    window_id,
                    event: win_event,
                    ..
                } => {
                    if let Some(win) = windows.get_mut(window_id) {
                        platform.handle_event(imgui.io_mut(), &win.win, &event);

                        // Ensure auxiliary windows (debugger/VRAM) stay responsive even if
                        // emulation frames are being dropped due to backpressure.
                        if !matches!(win.kind, WindowKind::Main)
                            && matches!(
                                win_event,
                                WindowEvent::CursorMoved { .. }
                                    | WindowEvent::MouseInput { .. }
                                    | WindowEvent::MouseWheel { .. }
                                    | WindowEvent::KeyboardInput { .. }
                            )
                        {
                            win.win.request_redraw();
                        }
                        if ui_state.show_context
                            && imgui.io().want_capture_mouse
                            && matches!(win_event, WindowEvent::MouseInput { .. })
                        {
                            return;
                        }
                        match win_event {
                            WindowEvent::CloseRequested => {
                                if matches!(win.kind, WindowKind::Main) {
                                    if !sent_shutdown {
                                        let _ =
                                            to_emu_tx.send(UiToEmu::Command(EmuCommand::Shutdown));
                                        sent_shutdown = true;
                                    }
                                    target.exit();
                                    #[allow(clippy::needless_return)]
                                    return;
                                } else {
                                    windows.remove(window_id);
                                }
                            }
                            WindowEvent::Resized(size) => {
                                win.resize(*size);
                                win.win.request_redraw();
                            }
                            WindowEvent::ScaleFactorChanged { .. } => {
                                // winit 0.30 provides an InnerSizeWriter; the current physical
                                // size can be queried from the window.
                                win.resize(win.win.inner_size());
                                win.win.request_redraw();
                            }
                            WindowEvent::CursorMoved { position, .. }
                                if matches!(win.kind, WindowKind::Main) =>
                            {
                                cursor_pos = *position;
                            }
                            WindowEvent::MouseInput {
                                state: ElementState::Pressed,
                                button: MouseButton::Right,
                                ..
                            } if matches!(win.kind, WindowKind::Main) => {
                                if !ui_state.paused && cursor_in_screen(&win.win, cursor_pos) {
                                    ui_state.paused = true;
                                    ui_state.show_context = true;
                                    ui_state.ctx_pos = [cursor_pos.x as f32, cursor_pos.y as f32];
                                    let _ = to_emu_tx
                                        .send(UiToEmu::Command(EmuCommand::SetPaused(true)));
                                    win.win.request_redraw();
                                }
                            }
                            WindowEvent::MouseInput {
                                state: ElementState::Pressed,
                                button: MouseButton::Left,
                                ..
                            } if matches!(win.kind, WindowKind::Main) => {
                                if ui_state.show_context && imgui.io().want_capture_mouse {
                                    // Menu click handled by ImGui
                                } else if cursor_in_screen(&win.win, cursor_pos) {
                                    ui_state.paused = false;
                                    ui_state.show_context = false;
                                    let _ = to_emu_tx
                                        .send(UiToEmu::Command(EmuCommand::SetPaused(false)));
                                    win.win.request_redraw();
                                }
                            }
                            WindowEvent::KeyboardInput { event, .. }
                                if matches!(win.kind, WindowKind::Main) =>
                            {
                                if !(ui_state.paused || imgui.io().want_text_input)
                                    && let PhysicalKey::Code(code) = event.physical_key
                                {
                                    let pressed = event.state == ElementState::Pressed;
                                    let mask = if code == KeyCode::Space {
                                        speed.fast = pressed;
                                        speed.factor = if speed.fast { FF_MULT } else { 1.0 };
                                        let _ = to_emu_tx
                                            .send(UiToEmu::Command(EmuCommand::SetSpeed(speed)));
                                        None
                                    } else {
                                        match code {
                                            KeyCode::ArrowRight => Some(0x01),
                                            KeyCode::ArrowLeft => Some(0x02),
                                            KeyCode::ArrowUp => Some(0x04),
                                            KeyCode::ArrowDown => Some(0x08),
                                            KeyCode::KeyS => Some(0x10),
                                            KeyCode::KeyA => Some(0x20),
                                            KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(0x40),
                                            KeyCode::Enter => Some(0x80),
                                            KeyCode::Escape => {
                                                if pressed {
                                                    target.exit();
                                                }
                                                None
                                            }
                                            _ => None,
                                        }
                                    };
                                    if let Some(mask) = mask {
                                        if pressed {
                                            state &= !mask;
                                        } else {
                                            state |= mask;
                                        }
                                        let _ = to_emu_tx
                                            .send(UiToEmu::Command(EmuCommand::UpdateInput(state)));
                                    }
                                }
                            }
                            WindowEvent::RedrawRequested => {
                                // Ensure the Pixels surface matches the actual window size.
                                // In some multi-window + DPI scenarios we can miss/lose a resize event,
                                // which otherwise leads to wgpu validation panics on scissor.
                                win.ensure_surface_matches_window();

                                // Keep ImGui's global display size/scale consistent for this window.
                                sync_imgui_display_for_window(&mut imgui, &win.win);

                                if let Err(e) = platform.prepare_frame(imgui.io_mut(), &win.win) {
                                    error!("imgui prepare_frame failed: {e}");
                                    return;
                                }
                                let ui = imgui.frame();

                                match win.kind {
                                    WindowKind::Main => {
                                        build_ui(&mut ui_state, ui, mobile.is_some());
                                        draw_game_screen(&mut win.pixels, &frame);
                                    }
                                    WindowKind::Debugger => {
                                        let snap =
                                            ui_snapshot.read().unwrap_or_else(|e| e.into_inner());
                                        draw_debugger(&mut win.pixels, &snap, ui)
                                    }
                                    WindowKind::VramViewer => {
                                        let snap =
                                            ui_snapshot.read().unwrap_or_else(|e| e.into_inner());
                                        draw_vram(win, &snap, ui)
                                    }
                                }

                                platform.prepare_render(ui, &win.win);
                                let draw_data = imgui.render();

                                let (surface_w, surface_h) = win.surface_size();

                                let render_result =
                                    win.pixels.render_with(|encoder, render_target, context| {
                                        context.scaling_renderer.render(encoder, render_target);

                                        if draw_data.total_vtx_count > 0 {
                                            let mut rpass = encoder.begin_render_pass(
                                                &wgpu::RenderPassDescriptor {
                                                    label: Some("imgui_pass"),
                                                    color_attachments: &[Some(
                                                        wgpu::RenderPassColorAttachment {
                                                            view: render_target,
                                                            resolve_target: None,
                                                            ops: wgpu::Operations {
                                                                load: wgpu::LoadOp::Load,
                                                                store: true,
                                                            },
                                                        },
                                                    )],
                                                    depth_stencil_attachment: None,
                                                },
                                            );

                                            let (clip_x, clip_y, clip_w, clip_h) =
                                                context.scaling_renderer.clip_rect();

                                            // Clamp against the Pixels surface extent, not the OS window size.
                                            // The render_target passed by pixels is the surface texture view.
                                            let max_w = surface_w;
                                            let max_h = surface_h;

                                            if max_w == 0
                                                || max_h == 0
                                                || clip_w == 0
                                                || clip_h == 0
                                                || clip_x >= max_w
                                                || clip_y >= max_h
                                            {
                                                return Ok(());
                                            }

                                            let clip_x2 =
                                                (clip_x.saturating_add(clip_w)).min(max_w);
                                            let clip_y2 =
                                                (clip_y.saturating_add(clip_h)).min(max_h);
                                            let clip_w = clip_x2.saturating_sub(clip_x);
                                            let clip_h = clip_y2.saturating_sub(clip_y);

                                            if clip_w == 0 || clip_h == 0 {
                                                return Ok(());
                                            }

                                            rpass.set_scissor_rect(clip_x, clip_y, clip_w, clip_h);

                                            if let Err(e) = win.renderer.render_clamped(
                                                draw_data,
                                                win.pixels.queue(),
                                                win.pixels.device(),
                                                &mut rpass,
                                                [surface_w, surface_h],
                                            ) {
                                                error!("imgui render failed: {e}");
                                                // Don't crash the process; skip ImGui for this frame.
                                                return Ok(());
                                            }
                                        }
                                        Ok(())
                                    });
                                if let Err(e) = render_result {
                                    error!("Pixels render failed: {e}");
                                    target.exit();
                                }

                                if ui_state.spawn_debugger
                                    && !windows
                                        .values()
                                        .any(|w| matches!(w.kind, WindowKind::Debugger))
                                {
                                    spawn_debugger_window(
                                        target,
                                        &mut platform,
                                        &mut imgui,
                                        &mut windows,
                                    );
                                    ui_state.paused = true;
                                    let _ = to_emu_tx
                                        .send(UiToEmu::Command(EmuCommand::SetPaused(true)));
                                    ui_state.spawn_debugger = false;
                                }
                                if ui_state.spawn_vram
                                    && !windows
                                        .values()
                                        .any(|w| matches!(w.kind, WindowKind::VramViewer))
                                {
                                    spawn_vram_window(
                                        target,
                                        &mut platform,
                                        &mut imgui,
                                        &mut windows,
                                    );
                                    ui_state.paused = true;
                                    let _ = to_emu_tx
                                        .send(UiToEmu::Command(EmuCommand::SetPaused(true)));
                                    ui_state.spawn_vram = false;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Event::AboutToWait => {
                    let mut got_frame = false;

                    while let Ok(evt) = from_emu_frame_rx.try_recv() {
                        if let EmuEvent::Frame {
                            frame: mut incoming,
                            frame_index: _,
                        } = evt
                            && incoming.len() == frame.len()
                        {
                            std::mem::swap(&mut frame, &mut incoming);
                            let _ = frame_pool_tx.send_timeout(incoming, Duration::ZERO);
                            got_frame = true;
                        }
                    }

                    while let Ok(evt) = from_emu_serial_rx.try_recv() {
                        if let EmuEvent::Serial { data, frame_index } = evt
                            && !data.is_empty()
                        {
                            print!("[SERIAL {frame_index}] ");
                            for b in &data {
                                if b.is_ascii_graphic() || *b == b' ' {
                                    print!("{}", *b as char);
                                } else {
                                    print!("\\x{b:02X}");
                                }
                            }
                            println!();
                        }
                    }

                    if let Some(peripheral) = ui_state.pending_serial_peripheral.take() {
                        let _ = to_emu_tx.send(UiToEmu::Command(EmuCommand::SetSerialPeripheral(
                            peripheral,
                        )));
                    }

                    if let Some(action) = ui_state.pending_action.take() {
                        let _ = to_emu_tx.send(UiToEmu::Action(action));
                        ui_state.paused = false;
                        ui_state.show_context = false;
                        let _ = to_emu_tx.send(UiToEmu::Command(EmuCommand::SetPaused(false)));
                        got_frame = true;
                    }

                    if got_frame || ui_state.show_context {
                        for win in windows.values() {
                            win.win.request_redraw();
                        }
                    }
                }
                Event::LoopExiting => {
                    if !sent_shutdown {
                        let _ = to_emu_tx.send(UiToEmu::Command(EmuCommand::Shutdown));
                        sent_shutdown = true;
                    }
                    if let Some(handle) = emu_handle.take() {
                        let _ = handle.join();
                    }
                }
                _ => {}
            }
        });
    } else {
        let mut frame_count = 0u64;
        let start = Instant::now();
        let mut mobile_time_accum_ns: u128 = 0;
        let mobile_active = serial_peripheral == SerialPeripheral::MobileAdapter;
        'headless: loop {
            while !gb.mmu.ppu.frame_ready() {
                gb.cpu.step(&mut gb.mmu);
                if let Some(max) = cycle_limit
                    && gb.cpu.cycles >= max
                {
                    break 'headless;
                }
                if let Some(limit) = second_limit
                    && start.elapsed() >= limit
                {
                    break 'headless;
                }
            }

            frame.copy_from_slice(gb.mmu.ppu.framebuffer());
            gb.mmu.ppu.clear_frame_flag();

            if debug_enabled && frame_count.is_multiple_of(60) {
                let serial = gb.mmu.take_serial();
                if !serial.is_empty() {
                    print!("[SERIAL] ");
                    for b in &serial {
                        if b.is_ascii_graphic() || *b == b' ' {
                            print!("{}", *b as char);
                        } else {
                            print!("\\x{b:02X}");
                        }
                    }
                    println!();
                }

                println!("{}", gb.cpu.debug_state());
            }

            frame_count += 1;

            if mobile_active && let Some(mobile) = mobile.as_ref() {
                mobile_time_accum_ns += FRAME_TIME.as_nanos();
                let delta_ms = (mobile_time_accum_ns / 1_000_000) as u32;
                mobile_time_accum_ns %= 1_000_000;
                if delta_ms != 0
                    && let Ok(mut adapter) = mobile.lock()
                {
                    let _ = adapter.poll(delta_ms);
                }
            }

            if let Some(max) = frame_limit
                && frame_count >= max as u64
            {
                break;
            }
            if let Some(limit) = second_limit
                && start.elapsed() >= limit
            {
                break;
            }
        }

        if let Some(mobile) = mobile.as_ref()
            && let Ok(mut adapter) = mobile.lock()
        {
            let _ = adapter.stop();
        }
    }
}
