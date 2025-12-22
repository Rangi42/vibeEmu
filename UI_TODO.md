# UI TODO (vibe-emu-ui)

This file lists problems and improvement opportunities observed in the current desktop UI implementation.

**Legend**
- **P0**: correctness/stability (panic, runaway memory/CPU, data races, broken UX)
- **P1**: major UX/perf issues that affect most users
- **P2**: nice-to-have usability improvements
- **P3**: cleanup/maintenance

---

## P0 — Stability / Correctness

- ✅ **P0 COMPLETE:** Unbounded frame queue can grow without bound (memory risk).
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `run_emulator_thread()` sends `EmuEvent::Frame` on an unbounded `std::sync::mpsc::channel()`; UI drains via `try_recv()` in `Event::AboutToWait`.
  - **Problem:** If the UI thread can’t keep up (debugger open, GPU stall, OS hiccup), frames accumulate indefinitely.
  - **Improve:** Use a bounded channel (e.g., crossbeam channel) or a “latest-frame only” shared buffer (swap `Arc<[u32]>` / double-buffer) and drop intermediate frames.

- ✅ **P0 COMPLETE:** Frequent `unwrap/expect/panic` paths in UI/audio can crash the process.
  - **Where:**
    - `crates/vibe-emu-ui/src/main.rs`: window creation, `Pixels::new(..).expect("Pixels error")`, `platform.prepare_frame(...).unwrap()`, imgui `render(...).expect(...)`, mutex `.expect("... poisoned")`.
    - `crates/vibe-emu-ui/src/audio.rs`: `build_output_stream(...).unwrap()`, `panic!("Unsupported sample format")`.
    - `crates/vibe-emu-ui/src/ui/vram_viewer.rs`: texture id `.unwrap()`.
  - **Improve:** Return/propagate errors and degrade gracefully (disable feature, show in-UI message, or log + continue). Convert panics to `Result`/`Option` and handle at the callsite.

- ✅ **P0 COMPLETE:** Main loop forces `ControlFlow::Poll` and redraws all windows every tick (high idle CPU).
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `event_loop.run(...)` sets `ControlFlow::Poll`, and `Event::AboutToWait` calls `request_redraw()` on every window.
  - **Problem:** Busy polling can peg a core even when paused / no new frames.
  - **Improve:** Use `ControlFlow::Wait` / `WaitUntil` and request redraw only when:
    - a new frame arrives,
    - UI state changes,
    - window events occur.

- ✅ **P0 COMPLETE:** Input hit-testing uses a fixed SCALE constant, not the actual drawable rect.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `cursor_in_screen()`.
  - **Problem:** If the window is resized or DPI/scaling changes, “inside screen” detection becomes incorrect.
  - **Improve:** Compute the game-viewport rectangle from the `pixels` scaling renderer output (or track the final blit/scale) and test against that.

---

## P1 — Performance / Responsiveness

- ✅ **P1 COMPLETE:** Per-frame allocation + full framebuffer clone in emulator thread.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → inside `run_emulator_thread()`:
    - `frame_buf = Some(mmu.ppu.framebuffer().to_vec());`
  - **Problem:** Allocates and copies every frame; increases GC/allocator pressure and can hurt pacing.
  - **Improve:** Reuse a preallocated buffer, send a pooled buffer, or share a ring buffer with a “latest frame” pointer.

- ✅ **P1 COMPLETE:** UI consumes frames only during `AboutToWait` (can add latency).
  - **Where:** `Event::AboutToWait` drains `from_emu_rx.try_recv()`.
  - **Problem:** Depending on event cadence, frames may sit queued until the next `AboutToWait`.
  - **Improve:** Trigger a redraw immediately when a frame is received (via `EventLoopProxy` + custom user event), or switch to a channel integration approach that wakes the loop.

- ✅ **P1 COMPLETE:** Avoid per-redraw `u32`→RGBA conversion.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → emulator thread pre-converts to RGBA layout; `draw_game_screen()` does a fast copy.
  - **Fix:** Convert 0x00RRGGBB → a `u32` byte layout matching Pixels RGBA8, then `bytemuck::cast_slice` + `copy_from_slice`.

- ✅ **P1 COMPLETE:** VRAM viewer rebuilds textures frequently; tab switching can cause redundant work.
  - **Where:** `crates/vibe-emu-ui/src/ui/vram_viewer.rs`.
  - **Improve:** Consider caching per-tab timestamps/dirty flags more consistently and rebuilding only when the underlying data changes.

---

## P1 — UX / Interaction

- ✅ **P1 COMPLETE:** Add always-visible menu bar for critical actions.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `build_ui()`.
  - **Fix:** ImGui main menu bar exposes Load/Reset/Pause/Tools without needing right-click.

- ✅ **P1 COMPLETE:** Make pause/menu behavior consistent.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `WindowEvent::MouseInput` handlers.
  - **Fix:** Right-click opens the context menu without pausing; left-click closes the menu without implicitly resuming.

- ✅ **P1 COMPLETE:** Add basic configurable key bindings.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` + `crates/vibe-emu-ui/src/keybinds.rs`.
  - **Fix:** Supports `--keybinds <path>` or `VIBEEMU_KEYBINDS` to remap joypad + pause/fast-forward/quit.

---

## P2 — Cross-platform / Compatibility

- ✅ **P2 COMPLETE:** Make `rfd` backend selection target-specific (avoid unconditional `gtk3`).
  - **Where:** `crates/vibe-emu-ui/Cargo.toml`.
  - **Fix:** `gtk3` is now only enabled on Linux; Windows/macOS use `rfd` defaults.

- ✅ **P0 COMPLETE:** Avoid forcing an invalid backend cross-platform; add an explicit backend override.
  - **Where:** `crates/vibe-emu-ui/src/main.rs` → `configure_wgpu_backend()`.
  - **Fix:** Users can explicitly override via `--wgpu-backend auto|dx12|dx11|vulkan|metal|gl`.
    - If omitted: Windows defaults to `dx12` (for stability); other platforms default to auto selection.

---

## P3 — Cleanup / Maintenance

- ✅ **P3 COMPLETE:** Debugger/VRAM/Options windows use a 1×1 `Pixels` buffer as a carrier for ImGui.
  - **Where:** `spawn_debugger_window()` / `spawn_vram_window()` / `spawn_options_window()`.
  - **Fix:** Kept the 1×1 approach but documented the intent at the creation sites and centralized the `(1, 1)` size into a named constant.

- ✅ **P3 COMPLETE:** Remove `TextureId` unwraps in the VRAM viewer update paths.
  - **Where:** `crates/vibe-emu-ui/src/ui/vram_viewer.rs`.
  - **Fix:** Replaced `unwrap()` with `if let Some(tex_id) = ...` and rebuild logic.

- ✅ **P3 COMPLETE:** Logging and stdout prints are mixed (serial/debug).
  - **Where:** serial/CPU debug output in `crates/vibe-emu-ui/src/main.rs`, plus core diagnostics in `crates/vibe-emu-core/src/*`.
  - **Fix:** Route diagnostics through `log` + `env_logger` with a `--log-level` flag; release builds default to `off`.
