//! Pinned means docking to the nearest work-area edge, never disabling drag.

use crate::settings::{self, AssistantPanelPosition};
use tauri::{AppHandle, Manager};

pub(super) fn snap(x: i32, y: i32, w: i32, h: i32, area: (i32, i32, i32, i32)) -> (i32, i32) {
    let (x, y) = super::panel::clamp_origin_to_area(x, y, w, h, area);
    let (ax, ay, aw, ah) = area;
    let right = ax + (aw - w).max(0);
    let bottom = ay + (ah - h).max(0);
    [
        (x - ax, (ax, y)),
        (right - x, (right, y)),
        (y - ay, (x, ay)),
        (bottom - y, (x, bottom)),
    ]
    .into_iter()
    .min_by_key(|(distance, _)| *distance)
    .map(|(_, position)| position)
    .unwrap_or((x, y))
}

/// Snap immediately when the pin is enabled, preserving the monitor and position.
pub fn dock_panel(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(crate::window_labels::ASSISTANT) else {
            return;
        };
        let result = (|| {
            let Some(monitor) = window.current_monitor()? else {
                return Ok(());
            };
            let position = window.outer_position()?;
            let size = window.outer_size()?;
            let area = monitor.work_area();
            let (x, y) = snap(
                position.x,
                position.y,
                size.width as i32,
                size.height as i32,
                (
                    area.position.x,
                    area.position.y,
                    area.size.width as i32,
                    area.size.height as i32,
                ),
            );
            #[cfg(target_os = "windows")]
            {
                crate::overlay::set_window_bounds_physical(
                    &window,
                    x,
                    y,
                    size.width as i32,
                    size.height as i32,
                )
                .map_err(std::io::Error::other)?;
                super::native::windows::clip_window(&window);
            }
            #[cfg(not(target_os = "windows"))]
            window.set_position(tauri::PhysicalPosition { x, y })?;
            let mut settings = settings::get_settings(&handle);
            settings.assistant_panel_position = Some(AssistantPanelPosition {
                x,
                y,
                rel_x: ((x - area.position.x) as f64 / area.size.width.max(1) as f64)
                    .clamp(0.0, 1.0),
                rel_y: ((y - area.position.y) as f64 / area.size.height.max(1) as f64)
                    .clamp(0.0, 1.0),
                monitor_name: monitor.name().cloned(),
            });
            settings::write_settings(&handle, settings);
            Ok::<(), Box<dyn std::error::Error>>(())
        })();
        if let Err(error) = result {
            log::warn!("Assistant docking failed: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn snaps_to_each_nearest_edge_without_changing_the_other_axis() {
        let area = (0, 0, 1920, 1080);
        for (position, expected) in [
            ((10, 200), (0, 200)),
            ((1400, 200), (1480, 200)),
            ((500, 10), (500, 0)),
            ((500, 400), (500, 440)),
        ] {
            assert_eq!(
                super::snap(position.0, position.1, 440, 640, area),
                expected
            );
        }
    }

    #[test]
    fn docking_handles_negative_monitors_and_oversized_panels() {
        assert_eq!(
            super::snap(-1880, 200, 440, 640, (-1920, 0, 1920, 1080)),
            (-1920, 200)
        );
        assert_eq!(
            super::snap(200, 200, 4000, 2000, (0, 0, 1920, 1080)),
            (0, 0)
        );
        assert_eq!(super::snap(-20, -10, 440, 640, (0, 0, 1920, 1080)), (0, 0));
    }
}
