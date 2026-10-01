//! Where the Flow Bar docks on screen (FR-001-01, FR-001-08).
//!
//! `effective_edge` merges the persisted Flow Bar docking setting with the
//! legacy `overlay_position` top/bottom switch, and `docked_position` is the
//! pure math that turns a work area plus a relative offset (0–1 along the
//! edge) into a window position. The same function feeds both coordinate
//! paths: logical points (tao positioning on macOS/Linux) and physical pixels
//! (the Win32 `SetWindowPos` path on Windows) — callers pre-scale inputs.

use crate::settings::{AppSettings, FlowbarEdge, OverlayPosition};

/// Clearance below a top-docked overlay: generous on macOS (menu bar), a slim
/// 4 px gutter on Windows/Linux.
#[cfg(target_os = "macos")]
pub(crate) const OVERLAY_TOP_OFFSET: f64 = 46.0;
#[cfg(not(target_os = "macos"))]
pub(crate) const OVERLAY_TOP_OFFSET: f64 = 4.0;

/// FR-001-01: docked edges sit this far inside the work area — the bar floats
/// 8 px above the taskbar by default (bottom-center).
#[cfg(not(target_os = "macos"))]
pub(crate) const FLOWBAR_EDGE_MARGIN: f64 = 8.0;

/// macOS keeps a larger clearance from the bottom edge (Dock).
#[cfg(target_os = "macos")]
const FLOWBAR_EDGE_MARGIN_MACOS: f64 = 15.0;

/// Edge margin applied at Bottom/Left/Right docking.
pub(crate) fn edge_margin() -> f64 {
    #[cfg(target_os = "macos")]
    {
        FLOWBAR_EDGE_MARGIN_MACOS
    }
    #[cfg(not(target_os = "macos"))]
    {
        FLOWBAR_EDGE_MARGIN
    }
}

/// Screen edge the Flow Bar is docked to. `Top` only exists because the
/// legacy `overlay_position` setting still offers it; the v1 Flow Bar spec
/// (FR-001-08) docks to bottom/left/right and T-041 owns the drag gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// The effective docking edge: an explicit left/right Flow Bar dock wins;
/// otherwise the legacy `overlay_position` picks top vs. bottom (bottom is
/// the v1 default, FR-001-01).
pub(crate) fn effective_edge(settings: &AppSettings) -> BarEdge {
    match settings.flowbar_position_edge {
        FlowbarEdge::Left => BarEdge::Left,
        FlowbarEdge::Right => BarEdge::Right,
        FlowbarEdge::Bottom => match settings.overlay_position {
            OverlayPosition::Top => BarEdge::Top,
            OverlayPosition::Bottom => BarEdge::Bottom,
        },
    }
}

/// Top-left corner of a `win`-sized window docked to `edge` inside `area`
/// (`(x, y, width, height)`), at `offset` (0–1) along the docked edge.
///
/// All inputs share one unit space: logical points for the tao path, physical
/// pixels for the Win32 path — margins arrive pre-scaled by the caller.
///
/// FR-001-01 default: bottom edge, centered (`offset = 0.5`), `edge_margin`
/// inside the *work area* so the bar floats just above the taskbar.
pub(crate) fn docked_position(
    area: (f64, f64, f64, f64),
    win: (f64, f64),
    edge: BarEdge,
    offset: f64,
    top_margin: f64,
    edge_margin: f64,
) -> (f64, f64) {
    let (ax, ay, aw, ah) = area;
    let (w, h) = win;
    let t = offset.clamp(0.0, 1.0);
    match edge {
        BarEdge::Top => (ax + (aw - w) * t, ay + top_margin),
        BarEdge::Bottom => (ax + (aw - w) * t, ay + ah - h - edge_margin),
        BarEdge::Left => (ax + edge_margin, ay + (ah - h) * t),
        BarEdge::Right => (ax + aw - w - edge_margin, ay + (ah - h) * t),
    }
}
