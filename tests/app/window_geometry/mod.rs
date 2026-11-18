use super::{frame_needs_realign, maximized_geometry_needs_repair, restored_from_maximized};

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
fn detects_a_client_area_that_does_not_cover_the_window() {
    // 125 % DPI, maximized: the OS insets the client by the resize frame.
    assert!(frame_needs_realign(7, 7, 1536, 816, 1550, 830));
    // Restored: client and window coincide, nothing to do.
    assert!(!frame_needs_realign(0, 0, 1520, 848, 1520, 848));
    // A leftover horizontal inset alone still needs a re-align.
    assert!(frame_needs_realign(7, 0, 1520, 848, 1534, 848));
}
