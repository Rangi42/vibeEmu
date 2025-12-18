# UI TODO (vibe-emu-ui)

This file lists problems and improvement opportunities observed in the current desktop UI implementation.

**Legend**
- **P0**: correctness/stability (panic, runaway memory/CPU, data races, broken UX)
- **P1**: major UX/perf issues that affect most users
- **P2**: nice-to-have usability improvements
- **P3**: cleanup/maintenance

---

## P0 — Stability / Correctness

- **P0: Unbounded frame queue can grow without bound (memory risk).**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `run_emulator_thread()` sends `EmuEvent::Frame` on an unbounded `std::sync::mpsc::channel()`; UI drains via `try_recv()` in `Event::AboutToWait`.
  - **Problem:** If the UI thread can’t keep up (debugger open, GPU stall, OS hiccup), frames accumulate indefinitely.
  - **Improve:** Use a bounded channel (e.g., crossbeam channel) or a “latest-frame only” shared buffer (swap `Arc<[u32]>` / double-buffer) and drop intermediate frames.

- **P0: Frequent `unwrap/expect/panic` paths in UI/audio can crash the process.**
  - **Where:**
    - `crates/vibe-emu-ui/src/main.rs`: window creation, `Pixels::new(..).expect("Pixels error")`, `platform.prepare_frame(...).unwrap()`, imgui `render(...).expect(...)`, mutex `.expect("... poisoned")`.
    - `crates/vibe-emu-ui/src/audio.rs`: `build_output_stream(...).unwrap()`, `panic!("Unsupported sample format")`.
    - `crates/vibe-emu-ui/src/ui/vram_viewer.rs`: texture id `.unwrap()`.
  - **Improve:** Return/propagate errors and degrade gracefully (disable feature, show in-UI message, or log + continue). Convert panics to `Result`/`Option` and handle at the callsite.

- **P0: Main loop forces `ControlFlow::Poll` and redraws all windows every tick (high idle CPU).**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `event_loop.run(...)` sets `ControlFlow::Poll`, and `Event::AboutToWait` calls `request_redraw()` on every window.
  - **Problem:** Busy polling can peg a core even when paused / no new frames.
  - **Improve:** Use `ControlFlow::Wait` / `WaitUntil` and request redraw only when:
    - a new frame arrives,
    - UI state changes,
    - window events occur.

- **P0: Input hit-testing uses a fixed SCALE constant, not the actual drawable rect.**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `cursor_in_screen()`.
  - **Problem:** If the window is resized or DPI/scaling changes, “inside screen” detection becomes incorrect.
  - **Improve:** Compute the game-viewport rectangle from the `pixels` scaling renderer output (or track the final blit/scale) and test against that.

---

## P1 — Performance / Responsiveness

- **P1: Per-frame allocation + full framebuffer clone in emulator thread.**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → inside `run_emulator_thread()`:
    - `frame_buf = Some(mmu.ppu.framebuffer().to_vec());`
  - **Problem:** Allocates and copies every frame; increases GC/allocator pressure and can hurt pacing.
  - **Improve:** Reuse a preallocated buffer, send a pooled buffer, or share a ring buffer with a “latest frame” pointer.

- **P1: UI consumes frames only during `AboutToWait` (can add latency).**
  - **Where:** `Event::AboutToWait` drains `from_emu_rx.try_recv()`.
  - **Problem:** Depending on event cadence, frames may sit queued until the next `AboutToWait`.
  - **Improve:** Trigger a redraw immediately when a frame is received (via `EventLoopProxy` + custom user event), or switch to a channel integration approach that wakes the loop.

- **P1: Converting `u32` pixels to RGBA bytes each redraw.**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `draw_game_screen()`.
  - **Problem:** It’s small (160×144) but still pure CPU work every frame.
  - **Improve:** Consider storing the framebuffer in RGBA8888 already, or use a faster conversion path (SIMD / bytemuck if representation matches).

- **P1: VRAM viewer rebuilds textures frequently; tab switching can cause redundant work.**
  - **Where:** `crates/vibe-emu-ui/src/ui/vram_viewer.rs`.
  - **Improve:** Consider caching per-tab timestamps/dirty flags more consistently and rebuilding only when the underlying data changes.

---

## P1 — UX / Interaction

- **P1: Discoverability is low (critical actions hidden behind right-click).**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `build_ui()` context menu.
  - **Problem:** Loading ROM/reset/debugger/VRAM viewer are only accessible via right-click pause menu.
  - **Improve:** Provide a simple always-available menu bar (ImGui main menu) or a small overlay button/shortcut to open the menu.

- **P1: Pause/menu behavior is surprising (right-click pauses; left-click unpauses).**
  - **Where:** `WindowEvent::MouseInput` handlers.
  - **Improve:** Separate “open menu” from “pause” semantics, and make the behavior consistent even when already paused.

- **P1: No configurable key bindings; limited input device support.**
  - **Where:** `WindowEvent::KeyboardInput` mapping.
  - **Improve:** Add a basic keybind config (file/env) and optionally gamepad support via winit/gamepad crate.

---

## P2 — Cross-platform / Compatibility

- **P2: `rfd` is built with `gtk3` feature unconditionally.**
  - **Where:** `crates/vibe-emu-ui/Cargo.toml`.
  - **Risk:** This may be unnecessary or problematic depending on platform/toolchain; it’s Linux-oriented.
  - **Improve:** Gate `rfd` features per-platform (e.g., enable `gtk3` only on Linux) or use `rfd` defaults for each OS.

- **P2: Forcing `WGPU_BACKEND=dx12` may break some systems/drivers.**
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `configure_wgpu_backend()`.
  - **Improve:** Prefer a “best available backend” default; allow override but don’t force unless a known-bad backend is detected.

---

## P3 — Cleanup / Maintenance

- **P3: Debugger/VRAM windows use a 1×1 `Pixels` buffer as a carrier for ImGui.**
  - **Where:** `spawn_debugger_window()` / `spawn_vram_window()`.
  - **Problem:** It’s workable but non-obvious and makes resizing logic confusing.
  - **Improve:** Either render ImGui without a `Pixels` surface for those windows, or use a clear, consistent buffer size and document the intent.

- **P3: TextureId unwraps in VRAM viewer could be made more robust.**
  - **Where:** `crates/vibe-emu-ui/src/ui/vram_viewer.rs`.
  - **Improve:** Avoid `.unwrap()` and early-return if texture creation fails (or if renderer state resets).

- **P3: Logging and stdout prints are mixed (serial/debug).**
  - **Where:** serial output paths in both UI and emulator thread.
  - **Improve:** Route all diagnostics through `log` (or provide a UI console window) to avoid stalling stdout and interleaving output.
