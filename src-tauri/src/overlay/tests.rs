//! Unit tests for the Flow Bar overlay: monitor containment, Windows bounds
//! math (work area + docked edges), docked-position geometry, the
//! click-through hit-test predicate, and presence/dwell bookkeeping.

use super::geometry::{docked_position, effective_edge, BarEdge};
use super::positioning::is_mouse_within_monitor;
#[cfg(target_os = "windows")]
use super::win32::windows_overlay_bounds_from_area;
use super::*;
use tauri::{PhysicalPosition, PhysicalSize};

#[test]
fn monitor_hit_test_uses_half_open_physical_bounds() {
    let position = PhysicalPosition::new(-2560, -200);
    let size = PhysicalSize::new(2560, 1440);

    assert!(is_mouse_within_monitor((-2560, -200), &position, &size));
    assert!(is_mouse_within_monitor((-1, 1239), &position, &size));
    assert!(!is_mouse_within_monitor((0, 0), &position, &size));
    assert!(!is_mouse_within_monitor((-1, 1240), &position, &size));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_cursor_hit_test_does_not_scale_physical_monitor_bounds() {
    let position = PhysicalPosition::new(1920, 0);
    let size = PhysicalSize::new(3840, 2160);
    let cursor = (5000, 1000);

    assert!(is_mouse_within_monitor(cursor, &position, &size));

    // This is the old mixed-coordinate comparison. It excludes a cursor
    // that is visibly inside a secondary display running at 150%.
    let scale = 1.5;
    let logical_position = PhysicalPosition::new(
        (position.x as f64 / scale) as i32,
        (position.y as f64 / scale) as i32,
    );
    let logical_size = PhysicalSize::new(
        (size.width as f64 / scale) as u32,
        (size.height as f64 / scale) as u32,
    );
    assert!(!is_mouse_within_monitor(
        cursor,
        &logical_position,
        &logical_size
    ));
}

#[test]
fn mouse_in_bounds() {
    let position = PhysicalPosition::new(0, 0);
    let size = PhysicalSize::new(1920, 1080);

    assert!(is_mouse_within_monitor((960, 540), &position, &size));
    assert!(is_mouse_within_monitor((0, 0), &position, &size));
    // half-open: right/bottom edges are exclusive
    assert!(!is_mouse_within_monitor((1920, 540), &position, &size));
    assert!(!is_mouse_within_monitor((960, 1080), &position, &size));
}

#[test]
fn mouse_on_offset_monitor() {
    let position = PhysicalPosition::new(1920, 0);
    let size = PhysicalSize::new(2560, 1440);

    assert!(is_mouse_within_monitor((3200, 720), &position, &size));
    assert!(!is_mouse_within_monitor((1919, 720), &position, &size));
    assert!(!is_mouse_within_monitor((3000, 1600), &position, &size));
}

#[test]
fn mouse_on_negative_monitor() {
    let position = PhysicalPosition::new(-1920, -1080);
    let size = PhysicalSize::new(1920, 1080);

    assert!(is_mouse_within_monitor((-960, -540), &position, &size));
    assert!(is_mouse_within_monitor((-1920, -1080), &position, &size));
    assert!(!is_mouse_within_monitor((0, -540), &position, &size));
}

#[test]
fn mouse_on_scaled_monitor() {
    // A 3840x2160 monitor at 2x becomes 1920x1080 logical points.
    let position = PhysicalPosition::new(0, 0);
    let size = PhysicalSize::new(1920, 1080); // logical

    assert!(is_mouse_within_monitor((960, 540), &position, &size));
    assert!(!is_mouse_within_monitor((2000, 2000), &position, &size));
}

// ── docked_position (pure geometry, FR-001-01 / FR-001-08) ───────────────

#[test]
fn docked_bottom_center_is_default() {
    // FR-001-01: bottom-center, 8 px inside the work area.
    let (x, y) = docked_position(
        (0.0, 0.0, 1920.0, 1040.0),
        (288.0, 104.0),
        BarEdge::Bottom,
        0.5,
        4.0,
        8.0,
    );
    assert_eq!(x, (1920.0 - 288.0) / 2.0);
    assert_eq!(y, 1040.0 - 104.0 - 8.0);
}

#[test]
fn docked_offset_slides_along_edge() {
    // offset 0.25 puts the window a quarter of the free space in.
    let (x, _) = docked_position(
        (0.0, 0.0, 1920.0, 1040.0),
        (288.0, 104.0),
        BarEdge::Bottom,
        0.25,
        4.0,
        8.0,
    );
    assert_eq!(x, (1920.0 - 288.0) * 0.25);
    // offset is clamped to 0..=1.
    let (clamped, _) = docked_position(
        (0.0, 0.0, 1920.0, 1040.0),
        (288.0, 104.0),
        BarEdge::Bottom,
        1.7,
        4.0,
        8.0,
    );
    assert_eq!(clamped, 1920.0 - 288.0);
}

#[test]
fn docked_top_left_right() {
    let area = (100.0, 50.0, 1000.0, 700.0);
    let win = (288.0, 104.0);
    assert_eq!(
        docked_position(area, win, BarEdge::Top, 0.5, 4.0, 8.0),
        (100.0 + (1000.0 - 288.0) / 2.0, 50.0 + 4.0)
    );
    assert_eq!(
        docked_position(area, win, BarEdge::Left, 0.5, 4.0, 8.0),
        (100.0 + 8.0, 50.0 + (700.0 - 104.0) / 2.0)
    );
    assert_eq!(
        docked_position(area, win, BarEdge::Right, 1.0, 4.0, 8.0),
        (100.0 + 1000.0 - 288.0 - 8.0, 50.0 + (700.0 - 104.0) * 1.0)
    );
}

#[test]
fn effective_edge_prefers_flowbar_dock() {
    let mut settings = AppSettings::default();
    // Defaults: bottom dock + bottom overlay → Bottom.
    assert_eq!(effective_edge(&settings), BarEdge::Bottom);
    // Legacy top switch is honored while the dock stays auto.
    settings.overlay_position = crate::settings::OverlayPosition::Top;
    assert_eq!(effective_edge(&settings), BarEdge::Top);
    // An explicit left/right dock wins over overlay_position.
    settings.flowbar_position_edge = crate::settings::FlowbarEdge::Right;
    assert_eq!(effective_edge(&settings), BarEdge::Right);
}

// ── Windows placement: work area + docked edges (physical px) ───────────

#[cfg(target_os = "windows")]
#[test]
fn windows_overlay_bounds_dock_to_work_area_not_monitor() {
    // 1920x1080 monitor, 48 px taskbar → work area 1920x1032. FR-001-01: the
    // bar floats 8 px above the taskbar (i.e. inside the work area), centered.
    let work_area = (0, 0, 1920, 1032);
    assert_eq!(
        windows_overlay_bounds_from_area(
            work_area,
            1.0,
            1.0,
            FLOWBAR_WIDTH,
            FLOWBAR_HEIGHT,
            BarEdge::Bottom,
            0.5,
        ),
        (816, 920, 288, 104)
    );
    // Legacy top position: 4 px below the work-area top.
    assert_eq!(
        windows_overlay_bounds_from_area(
            work_area,
            1.0,
            1.0,
            FLOWBAR_WIDTH,
            FLOWBAR_HEIGHT,
            BarEdge::Top,
            0.5,
        ),
        (816, 4, 288, 104)
    );
    // Right dock at offset 0.5 → 8 px in from the work-area right edge.
    assert_eq!(
        windows_overlay_bounds_from_area(
            work_area,
            1.0,
            1.0,
            FLOWBAR_WIDTH,
            FLOWBAR_HEIGHT,
            BarEdge::Right,
            0.5,
        ),
        (1624, 464, 288, 104)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_overlay_bounds_use_destination_monitor_scale() {
    // Secondary 4K monitor at 150% DPI whose work area starts at x=1920 with a
    // 60 px taskbar: 3840x2160 monitor → work area 3840x2100 physical.
    let work_area = (1920, 0, 3840, 2100);
    let (x, y, w, h) = windows_overlay_bounds_from_area(
        work_area,
        1.5,
        1.0,
        FLOWBAR_WIDTH,
        FLOWBAR_HEIGHT,
        BarEdge::Bottom,
        0.5,
    );
    assert_eq!((w, h), (432, 156)); // 288x104 logical @ 1.5
    assert_eq!(x, 1920 + (3840 - 432) / 2);
    assert_eq!(y, 2100 - 156 - 12); // 8 px margin @ 1.5
}

#[cfg(target_os = "windows")]
#[test]
fn windows_overlay_bounds_grow_with_text_scale_without_moving_the_docked_edge() {
    let work_area = (-2560, -200, 2560, 1440);
    let (x, y, width, height) = windows_overlay_bounds_from_area(
        work_area,
        1.25,
        1.1,
        OVERLAY_STREAM_WIDTH,
        OVERLAY_STREAM_HEIGHT,
        BarEdge::Bottom,
        0.5,
    );
    // 400x120 logical at 1.25 DPI x 1.1 text = 550x165, still centered.
    assert_eq!((width, height), (550, 165));
    assert_eq!(x, -2560 + (2560 - 550) / 2);
    // Bottom edge = work-area bottom - 8 px @ 1.25 DPI margin.
    assert_eq!(y + height, 1440 - 200 - 10);
}

#[test]
fn compact_window_is_sized_for_hover_card_and_tooltip() {
    // Any non-streaming state shares the Flow Bar footprint — the window must
    // be big enough for the hover card plus its shortcut tooltip without a
    // resize during hover (F001: fixed size per visual class).
    assert_eq!(overlay_dimensions("idle"), (FLOWBAR_WIDTH, FLOWBAR_HEIGHT));
    assert_eq!(overlay_dimensions("error"), (FLOWBAR_WIDTH, FLOWBAR_HEIGHT));
    assert_eq!(
        overlay_dimensions("streaming"),
        (OVERLAY_STREAM_WIDTH, OVERLAY_STREAM_HEIGHT)
    );
}

// ── click-through hit-test (NFR-001-02 / AC-001-02..03) ─────────────────

#[test]
fn point_in_rect_respects_cushion_and_factor() {
    use crate::overlay::click_through::point_in_rect;
    let rect = FlowbarRect {
        x: 120.0,
        y: 96.0,
        width: 48.0,
        height: 8.0,
    };
    // Inside.
    assert!(point_in_rect(130.0, 100.0, 0.0, 0.0, rect, 1.0, 4.0));
    // Just inside the 4 px cushion.
    assert!(point_in_rect(118.0, 96.0, 0.0, 0.0, rect, 1.0, 4.0));
    // Outside beyond the cushion.
    assert!(!point_in_rect(110.0, 100.0, 0.0, 0.0, rect, 1.0, 4.0));
    // A 2x factor (Windows text scale) doubles the CSS space in cursor units.
    assert!(point_in_rect(240.0, 192.0, 0.0, 0.0, rect, 2.0, 4.0));
    assert!(!point_in_rect(239.0, 192.0, 0.0, 0.0, rect, 2.0, 0.0));
}

// ── presence / suppression (FR-001-07, FR-001-10) ───────────────────────

#[test]
fn always_on_requires_visible_overlay() {
    let mut settings = AppSettings::default();
    settings.flowbar_visibility = crate::settings::FlowbarVisibility::Always;
    assert!(flowbar_always_on(&settings));
    // overlay_style == None is the total kill switch (Linux default).
    settings.overlay_style = OverlayStyle::None;
    assert!(!flowbar_always_on(&settings));
    settings.overlay_style = OverlayStyle::Minimal;
    // DuringRecording is not always-on.
    settings.flowbar_visibility = crate::settings::FlowbarVisibility::DuringRecording;
    assert!(!flowbar_always_on(&settings));
    // Never suppresses everything, session states included.
    settings.flowbar_visibility = crate::settings::FlowbarVisibility::Never;
    assert!(flowbar_suppressed(&settings));
    assert!(!flowbar_always_on(&settings));
}

#[test]
fn snooze_suppresses_until_deadline() {
    let mut settings = AppSettings::default();
    assert!(!flowbar_snoozed(&settings));
    // Already-expired deadlines don't suppress.
    settings.flowbar_snoozed_until_ms = Some(1);
    assert!(!flowbar_snoozed(&settings));
    // A future one does.
    settings.flowbar_snoozed_until_ms = Some(crate::tray::now_unix_ms() + 60_000);
    assert!(flowbar_snoozed(&settings));
    assert!(flowbar_suppressed(&settings));
}

#[test]
fn session_dwell_deadline_follows_state() {
    SESSION_DWELL_UNTIL_MS.store(0, Ordering::SeqCst);
    note_session_state(r#"{"state":"done","session_id":"x","mode":"push_to_talk","pending":0}"#);
    let done_deadline = SESSION_DWELL_UNTIL_MS.load(Ordering::SeqCst);
    assert!(done_deadline > 0 && done_deadline <= now_unix_ms_u64() + 700);
    note_session_state(r#"{"state":"error","session_id":"x","mode":"push_to_talk","pending":0}"#);
    let err_deadline = SESSION_DWELL_UNTIL_MS.load(Ordering::SeqCst);
    assert!(err_deadline > done_deadline);
    // Any non-dwelling state clears the deadline.
    note_session_state(r#"{"state":"idle","session_id":null,"mode":"push_to_talk","pending":0}"#);
    assert_eq!(SESSION_DWELL_UNTIL_MS.load(Ordering::SeqCst), 0);
    // Garbage payloads are ignored rather than crashing the listener.
    note_session_state("not json");
    assert_eq!(SESSION_DWELL_UNTIL_MS.load(Ordering::SeqCst), 0);
}
