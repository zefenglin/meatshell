use super::{
    frame_needs_realign, maximized_geometry_needs_repair, restored_from_maximized, restored_origin,
};

#[test]
fn repairs_large_maximized_geometry_mismatch() {
    assert!(maximized_geometry_needs_repair(604, 1384, 1080, 1501));
    assert!(maximized_geometry_needs_repair(1920, 1000, 3840, 2160));
}

#[test]
fn accepts_taskbar_sized_maximized_work_area() {
    assert!(!maximized_geometry_needs_repair(1920, 1040, 1920, 1080));
    assert!(!maximized_geometry_needs_repair(2560, 1400, 2560, 1440));
}

#[test]
fn detects_the_maximize_restore_edge() {
    assert!(restored_from_maximized(true, false));
    assert!(!restored_from_maximized(false, true));
    assert!(!restored_from_maximized(false, false));
    assert!(!restored_from_maximized(true, true));
}

#[test]
fn keeps_the_restored_title_bar_on_the_screen() {
    // Maximized on a 1920x1080 screen at 125 %: outer rect (-9,-9)-(1929,1029),
    // normal size 1214x755, work area 1920x1032. The pointer starts 29 px below
    // the top edge, which puts the naive origin above the work area.
    let origin = restored_origin(
        (200, 20),
        (-9, -9, 1929, 1029),
        (1214, 755),
        (0, 0, 1920, 1032),
    );
    assert!(origin.1 >= 0, "title bar must stay on screen: {origin:?}");
    assert!(origin.1 + 755 <= 1032, "window must stay inside: {origin:?}");
    assert!(origin.0 >= 0 && origin.0 + 1214 <= 1920);
}

#[test]
fn keeps_the_restored_window_on_the_monitor_above() {
    // A second monitor stacked above the primary one: the clamp has to use that
    // monitor's work area, not the primary one.
    let origin = restored_origin(
        (400, -1050),
        (-9, -1089, 1929, -51),
        (1214, 755),
        (0, -1080, 1920, 0),
    );
    assert!(origin.1 >= -1080, "must stay on the upper monitor: {origin:?}");
    assert!(origin.1 + 755 <= 0);
}

#[test]
fn restores_under_the_pointer_inside_the_work_area() {
    // A drag that starts mid-screen keeps the pointer at the same relative spot.
    let origin = restored_origin(
        (960, 300),
        (-9, -9, 1929, 1029),
        (1214, 755),
        (0, 0, 1920, 1032),
    );
    // ratio_x = 969/1938 = 0.5, ratio_y = 309/1038; applied to 1214x755.
    assert_eq!(origin, (353, 75));
}

#[test]
fn detects_a_client_area_that_does_not_cover_the_window() {
    // 125 % DPI, maximized: the OS insets the client by the resize frame.
    assert!(frame_needs_realign(7, 7, 1536, 816, 1550, 830));
    // Restored: client and window coincide, nothing to do.
    assert!(!frame_needs_realign(0, 0, 1520, 848, 1520, 848));
    // A leftover horizontal inset alone still needs a re-align.
    assert!(frame_needs_realign(7, 0, 1520, 848, 1534, 848));
}
