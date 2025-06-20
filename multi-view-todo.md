# ✅ TODO ─ Multi-window refactor for `vibeEmu`

> **High-level goal:** keep a single `wgpu::Device/Queue` and a single `imgui::Context`, but attach that context to *three* `winit::window::Window`s and drive them from one event loop.

---

- [ ] **Create a per-window wrapper**

    ```rust
    struct UiWindow {
        window: winit::window::Window,
        surface_tex: pixels::SurfaceTexture,
        pixels: pixels::Pixels,
        kind: WindowKind,          // enum { Main, Debugger, Vram }
    }
    ```

---

- [ ] **Add a `windows: HashMap<WindowId, UiWindow>` to `main.rs`**

    Seed it with the existing main screen window (kind = `Main`).

---

- [ ] **Attach every new window to ImGui’s platform layer**

    ```rust
    platform.attach_window(imgui.io_mut(), &new_win.window, HiDpiMode::Rounded);
    ```

---

- [ ] **Create a window spawn helper function**

    ```rust
    fn spawn_debugger_window(event_loop: &winit::event_loop::EventLoopWindowTarget<()>,
                             device: &wgpu::Device,
                             queue: &wgpu::Queue,
                             platform: &mut WinitPlatform,
                             imgui: &mut imgui::Context,
                             windows: &mut HashMap<WindowId, UiWindow>) {
        let w = winit::window::WindowBuilder::new()
            .with_title("vibeEmu – Debugger")
            .with_inner_size(LogicalSize::new(640.0, 480.0))
            .build(event_loop).unwrap();

        let size = w.inner_size();
        let surface = pixels::SurfaceTexture::new(size.width, size.height, &w);
        let pixels = pixels::PixelsBuilder::new(1, 1, surface)
            .device(device.clone())
            .queue(queue.clone())
            .build().unwrap();

        platform.attach_window(imgui.io_mut(), &w, HiDpiMode::Rounded);

        windows.insert(w.id(), UiWindow {
            window: w,
            surface_tex: surface,
            pixels,
            kind: WindowKind::Debugger
        });
    }
    ```

    Repeat for VRAM viewer.

---

- [ ] **Update context menu to spawn windows on click**

    ```rust
    if ui.button("Debugger") && !windows.values().any(|w| w.kind == WindowKind::Debugger) {
        spawn_debugger_window(event_loop, device, queue, &mut platform, &mut imgui, &mut windows);
        state.paused = true;
    }
    ```

---

- [ ] **Implement centralized event loop logic**

    ```rust
    event_loop.run(move |event, target, cf| {
        match &event {
            Event::WindowEvent { window_id, event, .. } => {
                if let Some(win) = windows.get_mut(window_id) {
                    platform.handle_event(imgui.io_mut(), &win.window, event);
                    match event {
                        WindowEvent::CloseRequested => { windows.remove(window_id); return; }
                        WindowEvent::Resized(size)   => { win.pixels.resize_surface(size.width, size.height).ok(); }
                        _ => {}
                    }
                }
            }
            Event::RedrawRequested(window_id) => {
                if let Some(win) = windows.get_mut(window_id) {
                    platform.prepare_frame(imgui.io_mut(), &win.window).unwrap();
                    let ui = imgui.frame();

                    match win.kind {
                        WindowKind::Main => draw_game_screen(&mut win.pixels, &mut gb, &ui),
                        WindowKind::Debugger => draw_debugger(&mut win.pixels, &mut gb, &ui),
                        WindowKind::Vram => draw_vram(&mut win.pixels, &mut gb, &ui),
                    }

                    platform.prepare_render(&ui, &win.window);
                    let draw_data = ui.render();
                    renderer.render(win.pixels.device(), win.pixels.queue(), draw_data).unwrap();
                    win.pixels.render().unwrap();
                }
            }
            Event::MainEventsCleared => {
                for win in windows.values() { win.window.request_redraw(); }
            }
            _ => {}
        }
    });
    ```

---

- [ ] **Write per-window rendering helpers**

    ```rust
    fn draw_debugger(pixels: &mut Pixels, gb: &mut GameBoy, ui: &imgui::Ui) {
        let _frame = pixels.get_frame(); // required but unused
        show_register_table(ui, gb);
    }
    ```

---

- [ ] **Support resume-on-click in the main window**

    ```rust
    WindowEvent::MouseInput {
        state: ElementState::Pressed,
        button: MouseButton::Left,
        ..
    } => {
        state.paused = false;
    }
    ```

---

- [ ] **Ensure all windows share one device/queue**

    Use `.clone()` when setting up `PixelsBuilder` for secondary windows.

---

- [ ] **Avoid `ViewportsEnable` config flag**

    Don't use this:

    ```rust
    imgui.io_mut().config_flags |= imgui::ConfigFlags::VIEWPORTS_ENABLE;
    ```

    It's not fully supported with `imgui-wgpu` and may cause rendering errors.

---

### 💡 Why this works

- `WinitPlatform` handles multiple window attachments.
- Each `pixels::Pixels` gets its own `SurfaceTexture`, but shares the same `Device` and `Queue`.
- `imgui-wgpu` rendering is tied to per-window `draw_data` generated via `ui.render()`.

---

### 🔗 Reference

> https://github.com/bvssvni/imgui-winit-support/blob/master/examples/multi_window.rs
