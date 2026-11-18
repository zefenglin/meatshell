use super::*;

#[cfg(target_os = "linux")]
pub(super) fn set_window_icon(window: &AppWindow) {
    use i_slint_backend_winit::winit::window::Icon;
    const ICON_PNG: &[u8] = include_bytes!("../../assets/icon@512.png");
    let Ok(img) = image::load_from_memory(ICON_PNG) else {
        return;
    };
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    let Ok(icon) = Icon::from_rgba(rgba.into_raw(), w, h) else {
        return;
    };
    window
        .window()
        .with_winit_window(|ww| ww.set_window_icon(Some(icon)));
}

/// On Windows, keep the frameless Slint surface and the native hit-test surface
/// aligned. Some Win10 systems expose winit's undecorated-shadow compatibility
/// frame as a real non-client strip, which shifts hit testing (#193).
#[cfg(windows)]
pub(super) fn apply_window_chrome(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window.with_winit_window(|ww| {
        let Ok(handle) = ww.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        let hwnd = h.hwnd.get();

        // Pointer-width param instead of *const c_void: keeps this FFI
        // declaration free of core::ffi (a module older rustc releases
        // lack) while remaining ABI-identical on every supported target.
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: isize, cb: u32) -> i32;
        }
        // DWMWA_WINDOW_CORNER_PREFERENCE = 33, DWMWCP_ROUND = 2 (Windows 11+).
        const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
        const DWMWCP_ROUND: u32 = 2;
        // DWMWA_BORDER_COLOR = 34; DWMWA_COLOR_NONE = 0xFFFFFFFE hides the 1 px
        // system outline DWM draws around a frameless window once it is restored.
        const DWMWA_BORDER_COLOR: u32 = 34;
        const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
        unsafe {
            let pref: u32 = DWMWCP_ROUND;
            let corner_hr = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const u32 as isize,
                4,
            );
            let border: u32 = DWMWA_COLOR_NONE;
            let border_hr = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                &border as *const u32 as isize,
                4,
            );
            tracing::debug!(
                "window chrome applied: hwnd={hwnd:#x} corner_hr={corner_hr:#x} border_hr={border_hr:#x}"
            );
        }
    });
}

#[cfg(not(windows))]
pub(super) fn apply_window_chrome(_window: &slint::Window) {}

/// True on the maximize→restore edge.
pub(super) fn restored_from_maximized(was_maximized: bool, now_maximized: bool) -> bool {
    was_maximized && !now_maximized
}

/// The client rect must cover the window rect exactly. Anything else means the
/// renderer can still paint from an inset origin, leaving an unpainted strip
/// along the window edge.
pub(super) fn frame_needs_realign(
    dx: i32,
    dy: i32,
    client_w: u32,
    client_h: u32,
    outer_w: u32,
    outer_h: u32,
) -> bool {
    dx != 0 || dy != 0 || client_w != outer_w || client_h != outer_h
}

/// A frameless window still carries the `WS_THICKFRAME` resize border, so while
/// it is maximized the OS inflates the outer rect and insets the client area by
/// the frame width (7 px at 125 %). Dragging a maximized window back to normal
/// size can leave that inset behind: the native client rect is correct again but
/// the Slint surface keeps painting from the maximized offset, which shows up as
/// an unpainted strip along the window edge (white when a window is behind it).
/// Recompute the frame and feed the real client size back to Slint so the
/// surface is recreated at (0, 0).
#[cfg(windows)]
pub(super) fn realign_frameless_surface(window: &slint::Window, force: bool) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct WinPoint {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn GetClientRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn ClientToScreen(hwnd: isize, point: *mut WinPoint) -> i32;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn RedrawWindow(
            hwnd: isize,
            rect: *const std::ffi::c_void,
            region: *const std::ffi::c_void,
            flags: u32,
        ) -> i32;
    }
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_NOOWNERZORDER: u32 = 0x0200;
    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_FRAME: u32 = 0x0010;
    const RDW_ALLCHILDREN: u32 = 0x0080;
    const RDW_UPDATENOW: u32 = 0x0100;

    let measured = window.with_winit_window(|ww| {
        let handle = ww.window_handle().ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return None;
        };
        let hwnd = h.hwnd.get();
        unsafe {
            let mut outer = WinRect::default();
            let mut client = WinRect::default();
            let mut origin = WinPoint::default();
            if GetWindowRect(hwnd, &mut outer) == 0
                || GetClientRect(hwnd, &mut client) == 0
                || ClientToScreen(hwnd, &mut origin) == 0
            {
                return None;
            }
            let outer_w = (outer.right - outer.left).max(0) as u32;
            let outer_h = (outer.bottom - outer.top).max(0) as u32;
            let client_w = (client.right - client.left).max(0) as u32;
            let client_h = (client.bottom - client.top).max(0) as u32;
            let dx = origin.x - outer.left;
            let dy = origin.y - outer.top;
            tracing::debug!(
                target: "window_chrome",
                dx,
                dy,
                client_w,
                client_h,
                outer_w,
                outer_h,
                needs = frame_needs_realign(dx, dy, client_w, client_h, outer_w, outer_h),
                "measured frameless frame"
            );
            let needs = frame_needs_realign(dx, dy, client_w, client_h, outer_w, outer_h);
            if !needs && !force {
                return None;
            }
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOOWNERZORDER
                    | SWP_NOACTIVATE
                    | SWP_FRAMECHANGED,
            );
            let scale = ww.scale_factor().max(0.01) as f64;
            Some((
                hwnd,
                slint::LogicalSize::new(client_w as f32 / scale as f32, client_h as f32 / scale as f32),
                dx,
                dy,
            ))
        }
    });
    let Some(Some((hwnd, logical, dx, dy))) = measured else {
        return false;
    };
    tracing::debug!(
        target: "window_chrome",
        dx,
        dy,
        width = logical.width,
        height = logical.height,
        "realigned frameless surface after maximize/restore"
    );
    window.dispatch_event(slint::platform::WindowEvent::Resized { size: logical });
    window.request_redraw();
    // Make sure DWM picks the new frame up right away instead of waiting for
    // the next paint of an unrelated element.
    unsafe {
        RedrawWindow(
            hwnd,
            std::ptr::null(),
            std::ptr::null(),
            RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
    // `SWP_FRAMECHANGED` can drop the Win11 rounded-corner hint.
    apply_window_chrome(window);
    true
}

#[cfg(not(windows))]
pub(super) fn realign_frameless_surface(_window: &slint::Window, _force: bool) -> bool {
    false
}

/// Whether the primary mouse button is currently held down.
#[cfg(windows)]
pub(super) fn primary_button_down() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
    }
    const VK_LBUTTON: i32 = 0x01;
    unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0 }
}

#[cfg(not(windows))]
pub(super) fn primary_button_down() -> bool {
    false
}

/// Re-align after a drag restored a maximized window, but only once the drag is
/// over. Dragging a maximized window restores it at the *start* of the drag and
/// the move loop keeps pumping timers, so a re-align fired during the drag gets
/// undone by the OS when the drag ends — the faster the drag, the more of the
/// stale frame is left behind (unpainted strip along the top/left edge).
pub(super) fn schedule_restore_realign(weak: slint::Weak<AppWindow>, attempt: u32) {
    // ~7 s of drag at 120 ms per attempt; a drag cannot outlast that.
    if attempt > 60 {
        return;
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(120), move || {
        if primary_button_down() {
            schedule_restore_realign(weak, attempt + 1);
            return;
        }
        let Some(win) = weak.upgrade() else {
            return;
        };
        realign_frameless_surface(&win.window(), true);
        // The OS settles the frame one more time after the release; repeat once.
        let again = win.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(200), move || {
            if let Some(win) = again.upgrade() {
                realign_frameless_surface(&win.window(), true);
            }
        });
    });
}

#[cfg(windows)]
pub(super) fn setup_windows_platform(renderer_mode: &str) {
    use i_slint_backend_winit::winit::platform::windows::WindowAttributesExtWindows;

    let mut builder = i_slint_backend_winit::Backend::builder();
    let configured_renderer = match renderer_mode {
        "gpu" => Some("femtovg".to_owned()),
        "software" => Some("software".to_owned()),
        _ => None,
    };
    // Any explicit environment value wins, including plain "winit" (automatic
    // renderer selection). This keeps the existing diagnostic escape hatch.
    let env_backend = std::env::var("SLINT_BACKEND").ok();
    let renderer = match env_backend.as_deref() {
        Some(backend) => backend
            .strip_prefix("winit-")
            .filter(|renderer| !renderer.is_empty())
            .map(str::to_owned),
        None => configured_renderer,
    };
    if let Some(renderer) = renderer.as_ref() {
        builder = builder.with_renderer_name(renderer.clone());
    }
    tracing::info!(
        renderer_mode,
        renderer = renderer.as_deref().unwrap_or("auto"),
        source = if env_backend.is_some() {
            "SLINT_BACKEND"
        } else {
            "settings"
        },
        "initializing Windows renderer"
    );
    let backend = builder
        .with_window_attributes_hook(|attrs| {
            attrs.with_transparent(false).with_undecorated_shadow(false)
        })
        .build();

    match backend {
        Ok(backend) => {
            if slint::platform::set_platform(Box::new(backend)).is_err() {
                tracing::warn!("Windows winit backend was already initialized");
            }
        }
        Err(err) => tracing::warn!("failed to initialize Windows winit backend: {err}"),
    }
}

/// Linux renderer selection from Settings. Leave Slint in charge when the
/// environment explicitly selects a backend, including non-winit backends.
/// Automatic mode likewise keeps Slint's native backend/renderer selection.
#[cfg(target_os = "linux")]
pub(super) fn setup_linux_platform(renderer_mode: &str) {
    if let Some(env_backend) = std::env::var_os("SLINT_BACKEND") {
        tracing::info!(
            renderer_mode,
            renderer = %env_backend.to_string_lossy(),
            source = "SLINT_BACKEND",
            "initializing Linux renderer"
        );
        return;
    }

    let renderer = match renderer_mode {
        "gpu" => "femtovg",
        "software" => "software",
        _ => {
            tracing::info!(
                renderer_mode,
                renderer = "auto",
                source = "settings",
                "initializing Linux renderer"
            );
            return;
        }
    };

    tracing::info!(
        renderer_mode,
        renderer,
        source = "settings",
        "initializing Linux renderer"
    );
    match i_slint_backend_winit::Backend::builder()
        .with_renderer_name(renderer.to_owned())
        .build()
    {
        Ok(backend) => {
            if slint::platform::set_platform(Box::new(backend)).is_err() {
                tracing::warn!("Linux winit backend was already initialized");
            }
        }
        Err(err) => tracing::warn!("failed to initialize Linux winit backend: {err}"),
    }
}

pub(super) fn clamp_window_size_to_monitor(
    window: &slint::Window,
    preferred: Option<(f32, f32)>,
) -> Option<(f32, f32)> {
    use i_slint_backend_winit::winit::dpi::{LogicalPosition, LogicalSize};

    window.with_winit_window(|ww| {
        #[cfg(target_os = "linux")]
        {
            use i_slint_backend_winit::winit::platform::wayland::WindowExtWayland;

            // Wayland compositors own the final surface size. A
            // request_inner_size call is only advisory and KWin may configure a
            // different size, leaving Slint's rendered and input geometries out
            // of sync (#286). Let the compositor choose the startup size.
            if ww.xdg_toplevel().is_some() {
                return None;
            }
        }

        let scale = ww.scale_factor().max(0.01);
        // Before `Window::run()` makes the native window visible, winit often
        // has no current monitor yet. Falling back to the primary monitor lets
        // the persisted size actually apply during startup (#278).
        let monitor = ww.current_monitor().or_else(|| ww.primary_monitor())?;
        let monitor_size = monitor.size();
        let monitor_pos = monitor.position();
        let max_w = (monitor_size.width as f64 / scale - 16.0).max(1.0) as f32;
        let max_h = (monitor_size.height as f64 / scale - 16.0).max(1.0) as f32;
        let min_w = 720.0_f32.min(max_w);
        let min_h = 420.0_f32.min(max_h);
        let current = ww.inner_size();
        let current_w = (current.width as f64 / scale) as f32;
        let current_h = (current.height as f64 / scale) as f32;
        let (want_w, want_h) = preferred.unwrap_or((current_w, current_h));
        let target_w = want_w.clamp(min_w, max_w);
        let target_h = want_h.clamp(min_h, max_h);

        if (target_w - current_w).abs() > 0.5
            || (target_h - current_h).abs() > 0.5
            || preferred.is_some()
        {
            let _ = ww.request_inner_size(LogicalSize::new(target_w as f64, target_h as f64));
        }

        if (target_w - want_w).abs() > 0.5 || (target_h - want_h).abs() > 0.5 {
            let mon_w = monitor_size.width as f64 / scale;
            let mon_h = monitor_size.height as f64 / scale;
            let mon_x = monitor_pos.x as f64 / scale;
            let mon_y = monitor_pos.y as f64 / scale;
            ww.set_outer_position(LogicalPosition::new(
                mon_x + (mon_w - target_w as f64).max(0.0) / 2.0,
                mon_y + (mon_h - target_h as f64).max(0.0) / 2.0,
            ));
        }

        Some((target_w, target_h))
    })?
}

#[cfg(target_os = "linux")]
pub(super) fn is_wayland_window(window: &slint::Window) -> bool {
    use i_slint_backend_winit::winit::platform::wayland::WindowExtWayland;

    window
        .with_winit_window(|ww| ww.xdg_toplevel().is_some())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn is_wayland_window(_window: &slint::Window) -> bool {
    false
}

/// Detect the Windows mixed-DPI failure where the native maximized flag stays
/// set but the HWND keeps a much smaller geometry from the previous monitor.
/// Normal maximized work areas may be a little smaller because of the taskbar;
/// only a large mismatch is considered stale.
pub(super) fn maximized_geometry_needs_repair(
    window_width: u32,
    window_height: u32,
    monitor_width: u32,
    monitor_height: u32,
) -> bool {
    window_width.saturating_mul(4) < monitor_width.saturating_mul(3)
        || window_height.saturating_mul(4) < monitor_height.saturating_mul(3)
}

/// Ask the renderer to repaint after the window becomes visible again and, on
/// Windows, repair a stale maximized rectangle caused by crossing monitors with
/// different DPI scales (#272). The second redraw runs after the window manager
/// has applied the restore/maximize transition.
pub(super) fn refresh_revealed_main_window(weak: slint::Weak<AppWindow>) {
    let Some(win) = weak.upgrade() else { return };
    let repair = win
        .window()
        .with_winit_window(|ww| {
            ww.request_redraw();
            if !cfg!(windows) || !ww.is_maximized() {
                return false;
            }
            let Some(monitor) = ww.current_monitor() else {
                return false;
            };
            let outer = ww.outer_size();
            let screen = monitor.size();
            let stale = maximized_geometry_needs_repair(
                outer.width,
                outer.height,
                screen.width,
                screen.height,
            );
            if stale {
                tracing::warn!(
                    "repairing stale maximized geometry: window={}x{} monitor={}x{} scale={}",
                    outer.width,
                    outer.height,
                    screen.width,
                    screen.height,
                    ww.scale_factor(),
                );
                ww.set_maximized(false);
            }
            stale
        })
        .unwrap_or(false);

    let weak2 = weak.clone();
    slint::Timer::single_shot(std::time::Duration::from_millis(60), move || {
        if let Some(win) = weak2.upgrade() {
            win.window().with_winit_window(|ww| {
                if repair {
                    ww.set_maximized(true);
                }
                ww.request_redraw();
            });
        }
    });
}

#[cfg(test)]
#[path = "../../tests/app/window_geometry/mod.rs"]
mod mixed_dpi_window_tests;

/// Terminal fullscreen (#495) may only be entered while the active tab is a
/// terminal session (SSH, local shell, serial, …) — not the welcome page or an
/// editor tab, which would leave a chromeless window with nothing to show.
pub(super) fn term_fullscreen_allowed<'a>(
    active_tab: &str,
    mut terminal_ids: impl Iterator<Item = &'a str>,
) -> bool {
    !active_tab.is_empty() && active_tab != "welcome" && terminal_ids.any(|id| id == active_tab)
}

fn active_tab_is_terminal(win: &AppWindow) -> bool {
    let active = win.get_active_tab_id();
    let terms = win.get_terminals();
    let ids: Vec<SharedString> = (0..terms.row_count())
        .filter_map(|i| terms.row_data(i))
        .map(|t| t.id)
        .collect();
    term_fullscreen_allowed(active.as_str(), ids.iter().map(|id| id.as_str()))
}

/// Enter/leave terminal fullscreen (#495): borderless OS fullscreen plus the
/// `term-fullscreen` flag that hides all window chrome in app.slint. Entering
/// is refused unless a terminal tab is active.
pub(super) fn set_term_fullscreen(win: &AppWindow, on: bool) {
    if on && !active_tab_is_terminal(win) {
        return;
    }
    if win.get_term_fullscreen() != on {
        win.set_term_fullscreen(on);
    }
    if win.window().is_fullscreen() != on {
        win.window().set_fullscreen(on);
    }
}

#[cfg(test)]
#[path = "../../tests/app/term_fullscreen/mod.rs"]
mod term_fullscreen_tests;

#[cfg(target_os = "linux")]
pub(super) fn schedule_slint_pointer_ungrab<T>(weak: slint::Weak<T>)
where
    T: slint::ComponentHandle + 'static,
{
    // Linux window managers/compositors may consume the release event after a
    // system move/resize starts. If Slint keeps its press grab, the whole app
    // can remain stuck in move/resize cursor mode. A few deferred synthetic
    // releases cover Cinnamon/Mutter/KWin timing differences.
    for delay_ms in [0_u64, 16, 80, 200] {
        let weak2 = weak.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(delay_ms), move || {
            if let Some(w) = weak2.upgrade() {
                let win = w.window();
                win.dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(-1.0, -1.0),
                    button: slint::platform::PointerEventButton::Left,
                });
                win.dispatch_event(slint::platform::WindowEvent::PointerExited);
            }
        });
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn schedule_slint_pointer_ungrab<T>(_weak: slint::Weak<T>)
where
    T: slint::ComponentHandle + 'static,
{
}

/// macOS-only: install a custom winit backend that makes the native title bar
/// transparent and lets the window content render *under* it (fullSizeContentView).
/// The title bar then picks up the app's dark theme / wallpaper (`Theme.window-base`)
/// instead of showing a bright native bar in dark mode (#162 follow-up — immersive
/// title bar). The traffic-light buttons are left in place; the UI insets its top by
/// `titlebar-inset` so tabs don't hide behind them.
///
/// Must run before any window is created. We build the backend explicitly, which
/// would otherwise bypass the `SLINT_BACKEND` renderer override that exists as the
/// macOS software/FemtoVG/Skia selection (#108/#129) — so we re-honour it by hand.
#[cfg(target_os = "macos")]
pub(super) fn setup_macos_platform(renderer_mode: &str) {
    use i_slint_backend_winit::winit::platform::macos::WindowAttributesExtMacOS;

    let mut builder = i_slint_backend_winit::Backend::builder();
    // An explicit environment value wins, including plain "winit" (Slint's
    // automatic choice). Otherwise use the renderer selected in Settings.
    let env_backend = std::env::var("SLINT_BACKEND").ok();
    let renderer = match env_backend.as_deref() {
        Some(backend) => backend
            .strip_prefix("winit-")
            .filter(|renderer| !renderer.is_empty())
            .map(str::to_owned),
        None => Some(renderer_mode.to_owned()),
    };
    if let Some(renderer) = renderer.as_ref() {
        builder = builder.with_renderer_name(renderer.clone());
    }
    tracing::info!(
        renderer_mode,
        renderer = renderer.as_deref().unwrap_or("auto"),
        source = if env_backend.is_some() {
            "SLINT_BACKEND"
        } else {
            "settings"
        },
        "initializing macOS renderer"
    );
    builder = builder.with_window_attributes_hook(|attrs| {
        attrs
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
    });
    match builder.build() {
        Ok(backend) => {
            if slint::platform::set_platform(Box::new(backend)).is_err() {
                tracing::warn!("winit backend already set; immersive macOS titlebar disabled");
            }
        }
        Err(e) => {
            tracing::warn!("winit backend build failed ({e}); immersive macOS titlebar disabled")
        }
    }
}

/// Center the window on the primary monitor's work area (Windows).
#[cfg(windows)]
pub(super) fn center_window(win: &AppWindow) {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn SystemParametersInfoW(action: u32, uiparam: u32, pvparam: *mut Rect, winini: u32)
            -> i32;
    }
    const SPI_GETWORKAREA: u32 = 0x0030;

    let size = win.window().size(); // physical pixels
    let mut wa = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa, 0) };
    if ok == 0 {
        return;
    }
    let area_w = (wa.right - wa.left).max(0) as u32;
    let area_h = (wa.bottom - wa.top).max(0) as u32;
    let x = wa.left + ((area_w.saturating_sub(size.width)) / 2) as i32;
    let y = wa.top + ((area_h.saturating_sub(size.height)) / 2) as i32;
    win.window()
        .set_position(slint::PhysicalPosition::new(x, y));
}

#[cfg(not(windows))]
pub(super) fn center_window(_win: &AppWindow) {}

/// Bring a window to the front and give it keyboard focus: un-minimize it
/// and ask the OS for focus. Used when an OS entry point (taskbar jump list,
/// Dock menu, desktop action) opens a window while the app is sitting in the
/// background. Best effort — strict environments (Wayland, the Windows
/// foreground lock) may degrade to a taskbar flash.
pub(super) fn raise_to_front(win: &AppWindow) {
    let _ = win.window().with_winit_window(|w| {
        w.set_minimized(false);
        w.focus_window();
    });
}

/// Confirm-close dialog and custom title-bar window controls (#119).
pub(super) fn wire_window_controls(
    ctx: &WinCtx,
    dock_stacks: &Rc<RefCell<DockStacks>>,
    exit_confirmed: &Rc<Cell<bool>>,
) {
    let WinCtx {
        core,
        store,
        registry,
        handles,
        sftp_handles,
        window,
        proc_win,
        sys_win,
        editor_win,
        ..
    } = ctx;
    let window_id = ctx.window_id;
    // Confirm-close dialog "Close" → actually quit the event loop (#88).
    {
        let weak = window.as_weak();
        let proc_weak = proc_win.as_weak();
        let sys_weak = sys_win.as_weak();
        let cc_store = store.clone();
        let cc_ds = dock_stacks.clone();
        let close_handles = handles.clone();
        let close_sftp_handles = sftp_handles.clone();
        let editor_weak = editor_win.as_weak();
        let close_exit_confirmed = exit_confirmed.clone();
        let close_registry = registry.clone();
        let close_core = core.clone();
        window.on_confirm_close_yes(move || {
            // Guard against a double click and against another close request
            // arriving from Windows Installer while shutdown is in progress.
            if close_exit_confirmed.replace(true) {
                return;
            }
            if let Some(w) = weak.upgrade() {
                w.set_confirm_close_open(false);
                save_layout(&w, &cc_store, &cc_ds);
                clear_zen_on_close(&w, &cc_store);
                let _ = w.hide();
            }
            // Ask every worker to stop before the runtime/event loop is torn
            // down, and hide the detachable monitor windows. Clearing the maps
            // also makes any repeated close request see no live sessions and
            // pass through immediately.
            teardown_window(
                window_id,
                &close_handles,
                &close_sftp_handles,
                &proc_weak,
                &sys_weak,
                &editor_weak,
            );
            if close_registry.unregister(window_id) {
                let _ = slint::quit_event_loop();
            }
            forget_window_state(&close_core, window_id);
        });
    }

    // --- Custom title-bar window controls (#119) --------------------------
    {
        let weak = window.as_weak();
        window.on_win_minimize(move || {
            if let Some(w) = weak.upgrade() {
                w.window().with_winit_window(|ww| ww.set_minimized(true));
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_win_maximize_toggle(move || {
            if let Some(w) = weak.upgrade() {
                let now = w.window().with_winit_window(|ww| {
                    let m = !ww.is_maximized();
                    ww.set_maximized(m);
                    m
                });
                if let Some(m) = now {
                    w.set_window_maximized(m);
                    if !m {
                        // The OS finishes restoring the frame after the click, so
                        // defer the re-align until the window has settled.
                        schedule_restore_realign(w.as_weak(), 0);
                    }
                }
            }
        });
    }
    // --- Terminal fullscreen (#495) ----------------------------------------
    {
        let weak = window.as_weak();
        window.on_toggle_term_fullscreen(move || {
            if let Some(w) = weak.upgrade() {
                set_term_fullscreen(&w, !w.get_term_fullscreen());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_term_fullscreen_check(move || {
            if let Some(w) = weak.upgrade() {
                if w.get_term_fullscreen() && !active_tab_is_terminal(&w) {
                    set_term_fullscreen(&w, false);
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let close_handles = handles.clone();
        let close_sftp_handles = sftp_handles.clone();
        let wc_proc_weak = proc_win.as_weak();
        let wc_sys_weak = sys_win.as_weak();
        let wc_editor_weak = editor_win.as_weak();
        let wc_store = store.clone();
        let wc_ds = dock_stacks.clone();
        let wc_exit_confirmed = exit_confirmed.clone();
        let wc_registry = registry.clone();
        let wc_core = core.clone();
        window.on_win_close(move || {
            if let Some(w) = weak.upgrade() {
                if tray::available() {
                    save_layout(&w, &wc_store, &wc_ds);
                    tray::hide_window(&wc_core, window_id, &w);
                    return;
                }
                // Mirror the native-X behaviour: confirm if sessions are open.
                if !should_block_close(wc_exit_confirmed.get(), !close_handles.borrow().is_empty())
                {
                    wc_exit_confirmed.set(true);
                    save_layout(&w, &wc_store, &wc_ds);
                    clear_zen_on_close(&w, &wc_store);
                    // Tear down this window's workers and hide its monitor
                    // windows; quit only if it was the last one.
                    teardown_window(
                        window_id,
                        &close_handles,
                        &close_sftp_handles,
                        &wc_proc_weak,
                        &wc_sys_weak,
                        &wc_editor_weak,
                    );
                    let _ = w.hide();
                    if wc_registry.unregister(window_id) {
                        let _ = slint::quit_event_loop();
                    }
                    forget_window_state(&wc_core, window_id);
                } else {
                    w.set_confirm_close_open(true);
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_win_drag(move || {
            if let Some(w) = weak.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_window();
                });
                schedule_slint_pointer_ungrab(weak.clone());
            }
        });
    }
    {
        use i_slint_backend_winit::winit::window::ResizeDirection;
        let weak = window.as_weak();
        window.on_win_resize(move |dir: i32| {
            if let Some(w) = weak.upgrade() {
                let d = match dir {
                    0 => ResizeDirection::North,
                    1 => ResizeDirection::South,
                    2 => ResizeDirection::East,
                    3 => ResizeDirection::West,
                    4 => ResizeDirection::NorthEast,
                    5 => ResizeDirection::NorthWest,
                    6 => ResizeDirection::SouthEast,
                    _ => ResizeDirection::SouthWest,
                };
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_resize_window(d);
                });
                schedule_slint_pointer_ungrab(weak.clone());
            }
        });
    }
}
