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
        fn IsZoomed(hwnd: isize) -> i32;
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
            // A maximized frameless window legitimately keeps the frame inset,
            // and re-setting its frame there breaks the maximized layout.
            if IsZoomed(hwnd) != 0 {
                return None;
            }
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
    // DWM clips the presented frame to the "extended frame bounds"; when that
    // still carries the maximized inset, the border area stays transparent.
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: isize, attr: u32, pv: isize, cb: u32) -> i32;
    }
    const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
    let mut dwm_bounds = [0i32; 4];
    let dwm_hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            dwm_bounds.as_mut_ptr() as isize,
            16,
        )
    };
    tracing::info!(
        target: "window_chrome",
        dx,
        dy,
        width = logical.width,
        height = logical.height,
        dwm_hr,
        dwm_l = dwm_bounds[0],
        dwm_t = dwm_bounds[1],
        dwm_r = dwm_bounds[2],
        dwm_b = dwm_bounds[3],
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

/// Whether the OS currently considers the window maximized (IsZoomed).
/// Authoritative on purpose: winit's cached flag can lag behind the OS during
/// the maximize-to-restore drag, and re-aligning a maximized window breaks its
/// layout.
#[cfg(windows)]
pub(super) fn is_window_zoomed(window: &slint::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "user32")]
    extern "system" {
        fn IsZoomed(hwnd: isize) -> i32;
    }
    window.with_winit_window(|ww| {
        let Ok(handle) = ww.window_handle() else {
            return false;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return false;
        };
        unsafe { IsZoomed(h.hwnd.get()) != 0 }
    })
    .unwrap_or(false)
}

#[cfg(not(windows))]
pub(super) fn is_window_zoomed(_window: &slint::Window) -> bool {
    false
}

/// Enter the OS move loop, but only while the primary button is really held.
///
/// `drag_window` sends `WM_NCLBUTTONDOWN`/`HTCAPTION` synchronously and the move
/// loop it enters ends on the button release. Entering it after the button is
/// already up leaves the loop waiting for an event that never comes, which
/// freezes the UI thread: the window then ignores every click, including the
/// next attempt to drag it.
#[cfg(windows)]
pub(super) fn safe_drag_window(window: &slint::Window) -> bool {
    if !primary_button_down() {
        tracing::info!(target: "window_chrome", "move loop skipped: primary button is up");
        return false;
    }
    let _ = window.with_winit_window(|ww| ww.drag_window());
    true
}

#[cfg(not(windows))]
pub(super) fn safe_drag_window(window: &slint::Window) -> bool {
    let _ = window.with_winit_window(|ww| ww.drag_window());
    true
}

/// Where a maximized window should be restored to when it is dragged back to
/// normal size: the pointer keeps the same relative spot on the window, and the
/// result is clamped into the monitor work area.
///
/// Clamping matters. A drag that starts near the top of the screen yields a
/// near-zero ratio, so the window lands with its top edge at (or just above)
/// the top of the desktop; if it is allowed to go further up, the custom title
/// bar leaves the screen and the window can no longer be grabbed at all.
pub(super) fn restored_origin(
    cursor: (i32, i32),
    maximized: (i32, i32, i32, i32),
    normal_size: (i32, i32),
    work: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (max_left, max_top, max_right, max_bottom) = maximized;
    let width = (max_right - max_left).max(1);
    let height = (max_bottom - max_top).max(1);
    let normal_width = normal_size.0.max(1);
    let normal_height = normal_size.1.max(1);
    let ratio_x = ((cursor.0 - max_left) as f32 / width as f32).clamp(0.0, 1.0);
    let ratio_y = ((cursor.1 - max_top) as f32 / height as f32).clamp(0.0, 1.0);
    let x = cursor.0 - (ratio_x * normal_width as f32).round() as i32;
    let y = cursor.1 - (ratio_y * normal_height as f32).round() as i32;
    let (work_left, work_top, work_right, work_bottom) = work;
    // A window larger than the work area stays pinned to its top-left corner.
    let max_x = (work_right - normal_width).max(work_left);
    let max_y = (work_bottom - normal_height).max(work_top);
    (x.clamp(work_left, max_x), y.clamp(work_top, max_y))
}

/// Diagnostic probe: report the composited pixels along the window edges next to
/// the native geometry. A strip that shows whatever is behind the window while
/// the client rect is already correct means the desktop compositor still holds
/// the maximized frame, not that the client rect is stale.
#[cfg(windows)]
pub(super) fn probe_window_edges(window: &slint::Window, tag: &str) {
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
    #[repr(C)]
    #[derive(Default)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        min_position: WinPoint,
        max_position: WinPoint,
        normal_position: WinRect,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn GetClientRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn ClientToScreen(hwnd: isize, point: *mut WinPoint) -> i32;
        fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> i32;
        fn IsZoomed(hwnd: isize) -> i32;
        fn GetWindowDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
    }
    #[repr(C)]
    #[derive(Default)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels_per_meter: i32,
        y_pels_per_meter: i32,
        clr_used: u32,
        clr_important: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(dc: isize) -> isize;
        fn CreateCompatibleBitmap(dc: isize, width: i32, height: i32) -> isize;
        fn SelectObject(dc: isize, object: isize) -> isize;
        fn BitBlt(
            dst: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            src: isize,
            src_x: i32,
            src_y: i32,
            rop: u32,
        ) -> i32;
        fn GetDIBits(
            dc: isize,
            bitmap: isize,
            start: u32,
            lines: u32,
            bits: *mut u8,
            info: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        fn DeleteObject(object: isize) -> i32;
        fn DeleteDC(dc: isize) -> i32;
    }
    const SRCCOPY: u32 = 0x00CC_0020;
    const DIB_RGB_COLORS: u32 = 0;

    let slint_size = window.size();
    let _ = window.with_winit_window(|ww| {
        let handle = ww.window_handle().ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return None;
        };
        let hwnd = h.hwnd.get();
        unsafe {
            let mut outer = WinRect::default();
            let mut client = WinRect::default();
            let mut origin = WinPoint::default();
            let mut placement = WindowPlacement::default();
            placement.length = std::mem::size_of::<WindowPlacement>() as u32;
            if GetWindowRect(hwnd, &mut outer) == 0
                || GetClientRect(hwnd, &mut client) == 0
                || ClientToScreen(hwnd, &mut origin) == 0
            {
                return None;
            }
            let _ = GetWindowPlacement(hwnd, &mut placement);
            let outer_w = (outer.right - outer.left).max(0);
            let outer_h = (outer.bottom - outer.top).max(0);
            let mut left_edge = String::new();
            let mut top_edge = String::new();
            // The screen DC cannot be blitted from this process (it fails with
            // ERROR_INVALID_HANDLE), so read the window's own surface instead:
            // the pixels the application painted along the very edge. An inset
            // client area shows up here as a strip that does not match the UI.
            let surface = GetWindowDC(hwnd);
            if surface != 0 {
                let strip = |src: isize, src_x: i32, src_y: i32, width: i32, height: i32| {
                    let mem = CreateCompatibleDC(src);
                    if mem == 0 {
                        return String::new();
                    }
                    let bitmap = CreateCompatibleBitmap(src, width, height);
                    let mut pixels = String::new();
                    if bitmap != 0 {
                        let previous = SelectObject(mem, bitmap);
                        if BitBlt(mem, 0, 0, width, height, src, src_x, src_y, SRCCOPY) != 0 {
                            let mut info = BitmapInfo::default();
                            info.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
                            info.header.width = width;
                            info.header.height = -height;
                            info.header.planes = 1;
                            info.header.bit_count = 32;
                            let mut buffer = vec![0u8; (width * height * 4) as usize];
                            if GetDIBits(
                                mem,
                                bitmap,
                                0,
                                height as u32,
                                buffer.as_mut_ptr(),
                                &mut info,
                                DIB_RGB_COLORS,
                            ) != 0
                            {
                                for pixel in buffer.chunks_exact(4) {
                                    pixels.push_str(&format!(
                                        "{:02x}{:02x}{:02x},",
                                        pixel[2], pixel[1], pixel[0]
                                    ));
                                }
                            }
                        }
                        SelectObject(mem, previous);
                        DeleteObject(bitmap);
                    }
                    DeleteDC(mem);
                    pixels
                };
                // Window DC coordinates are relative to the window rect.
                left_edge = strip(surface, 0, outer_h / 2, 12, 1);
                top_edge = strip(surface, outer_w / 2, 0, 1, 12);
                ReleaseDC(hwnd, surface);
            }
            let inner = ww.inner_size();
            tracing::info!(
                target: "window_chrome",
                tag,
                outer = %format!("{outer_w}x{outer_h}@({},{})", outer.left, outer.top),
                client = %format!(
                    "{}x{}@({},{})",
                    client.right - client.left,
                    client.bottom - client.top,
                    origin.x - outer.left,
                    origin.y - outer.top
                ),
                inner = %format!("{}x{}", inner.width, inner.height),
                slint = %format!("{}x{}", slint_size.width, slint_size.height),
                scale = ww.scale_factor(),
                zoomed = IsZoomed(hwnd) != 0,
                show_cmd = placement.show_cmd,
                left_edge = %left_edge,
                top_edge = %top_edge,
                "window edge probe"
            );
        }
        Some(())
    });
}

#[cfg(not(windows))]
pub(super) fn probe_window_edges(_window: &slint::Window, _tag: &str) {}

/// Force the desktop compositor to rebuild the window frame and surface. A
/// frameless window dragged out of the maximized state can keep compositing the
/// maximized frame even though every native rectangle already reports the
/// restored geometry; cloaking it for a moment makes DWM re-evaluate the window.
#[cfg(windows)]
pub(super) fn nudge_compositor(window: &slint::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: isize, cb: u32) -> i32;
    }
    // DWMWA_CLOAK = 13.
    const DWMWA_CLOAK: u32 = 13;
    window
        .with_winit_window(|ww| {
            let handle = ww.window_handle().ok()?;
            let RawWindowHandle::Win32(h) = handle.as_raw() else {
                return None;
            };
            let hwnd = h.hwnd.get();
            unsafe {
                let on: i32 = 1;
                let off: i32 = 0;
                let cloak = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &on as *const i32 as isize, 4);
                let reveal = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &off as *const i32 as isize, 4);
                tracing::info!(target: "window_chrome", cloak, reveal, "compositor cloak cycle");
            }
            Some(())
        })
        .is_some()
}

#[cfg(not(windows))]
pub(super) fn nudge_compositor(_window: &slint::Window) -> bool {
    false
}

/// Force the renderer to recreate its drawable at the current size. A frameless
/// window that comes back from the maximized state can keep painting into a
/// surface of the old size while every rectangle already reports the restored
/// geometry; a real resize makes the compositor and the renderer agree again.
#[cfg(windows)]
pub(super) fn nudge_window_size(window: &slint::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn IsZoomed(hwnd: isize) -> i32;
    }
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_NOOWNERZORDER: u32 = 0x0200;
    const SWP_FRAMECHANGED: u32 = 0x0020;

    window
        .with_winit_window(|ww| {
            let handle = ww.window_handle().ok()?;
            let RawWindowHandle::Win32(h) = handle.as_raw() else {
                return None;
            };
            let hwnd = h.hwnd.get();
            unsafe {
                if IsZoomed(hwnd) != 0 {
                    return None;
                }
                let mut rect = WinRect::default();
                if GetWindowRect(hwnd, &mut rect) == 0 {
                    return None;
                }
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                // FRAMECHANGED makes the window manager recompute the non-client
                // area; preserving it is what keeps the maximized inset alive.
                let flags =
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_FRAMECHANGED;
                let grown =
                    SetWindowPos(hwnd, 0, rect.left, rect.top, width + 3, height + 3, flags);
                let shrunk = SetWindowPos(hwnd, 0, rect.left, rect.top, width, height, flags);
                tracing::info!(
                    target: "window_chrome",
                    grown,
                    shrunk,
                    width,
                    height,
                    "surface size nudge"
                );
            }
            Some(())
        })
        .is_some()
}

#[cfg(not(windows))]
pub(super) fn nudge_window_size(_window: &slint::Window) -> bool {
    false
}


/// Re-align after a window resize settles (a maximize-to-restore in particular),
/// but only once the drag is over. The OS restores a dragged maximized window at
/// the *start* of the drag and its move loop keeps pumping timers, so a re-align
/// fired during the drag is undone when the drag ends: the stale frame then
/// stays on screen as a transparent strip along the window edge.
///
/// A resize drag emits a burst of resize events, so a pending flag makes sure
/// only one re-align chain runs per window.
pub(super) fn schedule_restore_realign(
    weak: slint::Weak<AppWindow>,
    pending: std::rc::Rc<std::cell::Cell<bool>>,
    attempt: u32,
) {
    if attempt == 0 {
        if pending.replace(true) {
            return;
        }
    } else if attempt > 60 {
        // ~7 s of drag at 120 ms per attempt; a drag cannot outlast that.
        pending.set(false);
        return;
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(120), move || {
        if primary_button_down() {
            schedule_restore_realign(weak, pending, attempt + 1);
            return;
        }
        let Some(win) = weak.upgrade() else {
            pending.set(false);
            return;
        };
        probe_window_edges(&win.window(), "release");
        realign_frameless_surface(&win.window(), true);
        probe_window_edges(&win.window(), "release-aligned");
        nudge_compositor(&win.window());
        // The window manager re-applies the restored frame a few more times
        // after the release, so keep re-aligning until it stops moving.
        settle_realign(win.as_weak(), pending, vec![200, 600, 1200, 2500], false);
    });
}

/// Repeat a forced re-align on a fixed schedule, clearing pending when done.
fn settle_realign(
    weak: slint::Weak<AppWindow>,
    pending: std::rc::Rc<std::cell::Cell<bool>>,
    remaining: Vec<u64>,
    nudged: bool,
) {
    let mut rest = remaining;
    if rest.is_empty() {
        pending.set(false);
        return;
    }
    let delay = rest.remove(0);
    slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
        if let Some(win) = weak.upgrade() {
            probe_window_edges(&win.window(), "settle");
            // Recreate the drawable once, right after the drag has settled: a
            // surface left over from the maximized state is the one thing a
            // frame-only repair cannot fix.
            if !nudged {
                nudge_window_size(&win.window());
                realign_frameless_surface(&win.window(), true);
            }
            // Only repair on later passes if the frame actually drifted again.
            realign_frameless_surface(&win.window(), false);
        }
        settle_realign(weak, pending, rest, true);
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

/// Debug-only harness for the maximize → drag → restore path.
///
/// `MEATSHELL_TEST_DRAG=old` drives the plain move loop (the behaviour before
/// the fix); any other value drives the restore-first path. The synthetic input
/// session this machine runs in cannot deliver clicks to the window, so the
/// page is started by the application itself and the modal loop is fed with
/// cursor moves from a helper thread.
#[cfg(all(windows, debug_assertions))]
fn spawn_test_drag_hook(weak: slint::Weak<AppWindow>) {
    let Ok(mode) = std::env::var("MEATSHELL_TEST_DRAG") else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = weak.upgrade() else { return };
            win.window()
                .with_winit_window(|ww| {
                    let _ = ww.set_maximized(true);
                });
            let weak2 = win.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(700), move || {
                let Some(win) = weak2.upgrade() else { return };
                let Some((left, top)) = window_origin(&win.window()) else {
                    return;
                };
                let base = (left + 120, top + 30);
                set_cursor(base.0, base.1);
                tracing::info!(target: "window_chrome", ?base, mode = %mode, "test drag: starting");
                // A real press is required: the move loop exits at once without
                // one, and so does the motion poll in the intercept.
                press_primary();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    for step in 1..=14 {
                        std::thread::sleep(std::time::Duration::from_millis(60));
                        set_cursor(base.0 + step * 30, base.1 + step * 22);
                    }
                });
                let weak3 = win.as_weak();
                let finished = std::rc::Rc::new(std::cell::Cell::new(false));
                let flag = finished.clone();
                slint::Timer::single_shot(
                    std::time::Duration::from_millis(1600),
                    move || {
                        if flag.get() {
                            return;
                        }
                        release_primary();
                        tracing::info!(target: "window_chrome", "test drag: released");
                        if let Some(win) = weak3.upgrade() {
                            probe_window_edges(&win.window(), "test-after");
                        }
                    },
                );
                if mode == "old" {
                    win.window().with_winit_window(|ww| {
                        let _ = ww.drag_window();
                    });
                } else {
                    restore_under_cursor_and_drag(&win.window());
                }
                finished.set(true);
                tracing::info!(target: "window_chrome", "test drag: loop returned");
                release_primary();
                let weak4 = win.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(900), move || {
                    if let Some(win) = weak4.upgrade() {
                        probe_window_edges(&win.window(), "test-after");
                    }
                });
            });
        });
    });
}

/// Screen origin of the window's outer rectangle.
#[cfg(all(windows, debug_assertions))]
fn window_origin(window: &slint::Window) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[repr(C)]
    #[derive(Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
    }
    window.with_winit_window(|ww| {
        let handle = ww.window_handle().ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return None;
        };
        let mut rect = WinRect::default();
        let ok = unsafe { GetWindowRect(h.hwnd.get(), &mut rect) };
        (ok != 0).then_some((rect.left, rect.top))
    })?
}

#[cfg(all(windows, debug_assertions))]
fn press_primary() {
    #[link(name = "user32")]
    extern "system" {
        fn mouse_event(flags: u32, dx: i32, dy: i32, data: u32, extra: usize);
    }
    unsafe {
        mouse_event(0x0002, 0, 0, 0, 0);
    }
}

#[cfg(all(windows, debug_assertions))]
fn release_primary() {
    #[link(name = "user32")]
    extern "system" {
        fn mouse_event(flags: u32, dx: i32, dy: i32, data: u32, extra: usize);
    }
    unsafe {
        mouse_event(0x0004, 0, 0, 0, 0);
    }
}

#[cfg(all(windows, debug_assertions))]
fn set_cursor(x: i32, y: i32) {
    #[link(name = "user32")]
    extern "system" {
        fn SetCursorPos(x: i32, y: i32) -> i32;
    }
    unsafe {
        SetCursorPos(x, y);
    }
}

/// Screen position of the mouse pointer.
#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    #[repr(C)]
    #[derive(Default)]
    struct WinPoint {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut WinPoint) -> i32;
    }
    let mut point = WinPoint::default();
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// Restore a maximized window and start the move loop only once the pointer has
/// actually moved.
///
/// The window manager restores a maximized window inside its own move loop, and
/// that transition leaves a frameless window with the maximized frame — the
/// transparent strip. Restoring first and *then* handing over to the move loop
/// (the transition the maximize button uses, which is clean) avoids it, but the
/// decision has to wait for real motion so a plain click still does nothing.
#[cfg(windows)]
pub(super) fn arm_maximized_drag(
    weak: slint::Weak<AppWindow>,
    press: Option<(i32, i32)>,
    attempt: u32,
    state: std::rc::Rc<std::cell::Cell<u8>>,
) {
    const POLL_MS: u64 = 60;
    const MAX_ATTEMPTS: u32 = 15;
    // A few pixels of slack: a shaky click is not a drag.
    const THRESHOLD: i32 = 5;
    let press = match press {
        Some(press) => press,
        None => {
            let Some(press) = cursor_pos() else {
                return;
            };
            press
        }
    };
    if attempt > MAX_ATTEMPTS || state.get() != 0 {
        return;
    }
    if attempt == 0 {
        tracing::info!(target: "window_chrome", press = ?press, "arming maximized drag");
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(POLL_MS), move || {
        if state.get() != 0 {
            return;
        }
        let down = primary_button_down();
        let now = cursor_pos();
        if attempt < 4 {
            tracing::info!(target: "window_chrome", attempt, down, now = ?now, "drag arm poll");
        }
        if !down {
            return;
        }
        let Some(now) = now else {
            return;
        };
        if (now.0 - press.0).abs() < THRESHOLD && (now.1 - press.1).abs() < THRESHOLD {
            arm_maximized_drag(weak, Some(press), attempt + 1, state);
            return;
        }
        tracing::info!(target: "window_chrome", attempt, ?now, "drag motion detected");
        state.set(1);
        if let Some(win) = weak.upgrade() {
            restore_under_cursor_and_drag(&win.window());
        }
    });
}

#[cfg(not(windows))]
pub(super) fn arm_maximized_drag(
    _weak: slint::Weak<AppWindow>,
    _press: Option<(i32, i32)>,
    _attempt: u32,
    _state: std::rc::Rc<std::cell::Cell<u8>>,
) {
}

/// Restore a maximized frameless window under the cursor and then hand the
/// window to the OS move loop.
///
/// Letting the window manager restore the window as part of the drag leaves the
/// frameless window with the maximized frame, so the surface ends up inset and a
/// transparent strip shows along the left/top edge. Restoring first — the same
/// transition the maximize button performs, which is known to be clean — keeps
/// that code path out of the way entirely.
#[cfg(windows)]
pub(super) fn restore_under_cursor_and_drag(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WinPoint {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        min_position: WinPoint,
        max_position: WinPoint,
        normal_position: WinRect,
    }
    impl Default for WindowPlacement {
        fn default() -> Self {
            Self {
                length: std::mem::size_of::<Self>() as u32,
                flags: 0,
                show_cmd: 0,
                min_position: WinPoint::default(),
                max_position: WinPoint::default(),
                normal_position: WinRect::default(),
            }
        }
    }
    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: WinRect,
        rc_work: WinRect,
        dw_flags: u32,
    }
    // MONITOR_DEFAULTTONEAREST
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut WinPoint) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut WinRect) -> i32;
        fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> i32;
        fn SetWindowPlacement(hwnd: isize, placement: *const WindowPlacement) -> i32;
        fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }
    // SW_SHOWNORMAL
    const SW_NORMAL: u32 = 1;

    /// Work area (screen minus taskbar) of the monitor the window sits on.
    fn work_area(hwnd: isize) -> Option<WinRect> {
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if monitor == 0 {
                return None;
            }
            let mut info = MonitorInfo {
                cb_size: std::mem::size_of::<MonitorInfo>() as u32,
                rc_monitor: WinRect::default(),
                rc_work: WinRect::default(),
                dw_flags: 0,
            };
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            Some(info.rc_work)
        }
    }

    let _ = window.with_winit_window(|ww| {
        let handle = ww.window_handle().ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return None;
        };
        let hwnd = h.hwnd.get();
        unsafe {
            // The move loop below has no release to consume once the button is
            // up, so it would never return. Nothing here is worth doing then.
            if !primary_button_down() {
                tracing::info!(
                    target: "window_chrome",
                    "restore under cursor skipped: primary button released"
                );
                return None;
            }
            let mut cursor = WinPoint::default();
            let mut maximized = WinRect::default();
            let mut placement = WindowPlacement::default();
            if GetCursorPos(&mut cursor) == 0
                || GetWindowRect(hwnd, &mut maximized) == 0
                || GetWindowPlacement(hwnd, &mut placement) == 0
            {
                return None;
            }
            let normal = placement.normal_position;
            let normal_width = (normal.right - normal.left).max(1);
            let normal_height = (normal.bottom - normal.top).max(1);
            // The maximized rect already covers the work area of the monitor the
            // window is on, which is a serviceable fallback if the query fails.
            let work = work_area(hwnd).unwrap_or(maximized);
            let (x, y) = restored_origin(
                (cursor.x, cursor.y),
                (maximized.left, maximized.top, maximized.right, maximized.bottom),
                (normal_width, normal_height),
                (work.left, work.top, work.right, work.bottom),
            );
            let mut target = placement;
            target.length = std::mem::size_of::<WindowPlacement>() as u32;
            target.show_cmd = SW_NORMAL;
            target.normal_position = WinRect {
                left: x,
                top: y,
                right: x + normal_width,
                bottom: y + normal_height,
            };
            let applied = SetWindowPlacement(hwnd, &target);
            tracing::info!(
                target: "window_chrome",
                applied,
                x,
                y,
                normal_width,
                normal_height,
                "restored maximized window under the cursor"
            );
            if applied == 0 {
                return None;
            }
            if primary_button_down() {
                let _ = ww.drag_window();
            } else {
                tracing::info!(
                    target: "window_chrome",
                    "move loop skipped after restore: primary button released"
                );
            }
            Some(())
        }
    });
}

#[cfg(not(windows))]
pub(super) fn restore_under_cursor_and_drag(window: &slint::Window) {
    let _ = window.with_winit_window(|ww| ww.drag_window());
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
    //
    // Deliberately Linux-only: on Windows the release does reach Slint, and a
    // synthetic one lands outside the pressed item (so it emits no click) while
    // still clearing `pressed` and the click counter — which kills the click
    // that follows and with it double-click-to-maximize.
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
                        schedule_restore_realign(
                            w.as_weak(),
                            std::rc::Rc::new(std::cell::Cell::new(false)),
                            0,
                        );
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
        #[cfg(all(windows, debug_assertions))]
        let hook_weak = weak.clone();
        window.on_win_drag(move || {
            if let Some(w) = weak.upgrade() {
                let maximized = is_window_zoomed(&w.window());
                tracing::info!(target: "window_chrome", maximized, "win-drag invoked");
                if maximized {
                    // Restore first, then drag: the OS restore during a drag is
                    // what leaves the frameless frame stale (transparent strip).
                    // 0 = still waiting, 1 = the restore happened, 2 = gave up.
                    let state = std::rc::Rc::new(std::cell::Cell::new(0u8));
                    arm_maximized_drag(weak.clone(), None, 0, state.clone());
                    let fallback = weak.clone();
                    slint::Timer::single_shot(
                        std::time::Duration::from_millis(900),
                        move || {
                            // Nothing arm-like happened: fall back to the plain
                            // move loop so dragging never stops working.
                            if state.get() == 0 {
                                state.set(2);
                                tracing::info!(
                                    target: "window_chrome",
                                    "drag intercept idle, falling back to the move loop"
                                );
                                // Only while the button is still held: the
                                // loop would otherwise hang the UI thread.
                                if let Some(win) = fallback.upgrade() {
                                    safe_drag_window(&win.window());
                                }
                            }
                        },
                    );
                } else {
                    safe_drag_window(&w.window());
                }
                schedule_slint_pointer_ungrab(weak.clone());
            }
        });
        #[cfg(all(windows, debug_assertions))]
        {
            spawn_test_drag_hook(hook_weak);
        }
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
