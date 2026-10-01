use super::*;

struct FixedSource(Option<i32>);
impl NotificationStateSource for FixedSource {
    fn raw_state(&self) -> Option<i32> {
        self.0
    }
}

const AREA: (f64, f64, f64, f64) = (0.0, 0.0, 1920.0, 1040.0);

#[test]
fn suppression_matches_spec_states() {
    assert!(suppresses_toasts(Some(QUNS_BUSY)));
    assert!(suppresses_toasts(Some(QUNS_RUNNING_D3D_FULL_SCREEN)));
    assert!(suppresses_toasts(Some(QUNS_PRESENTATION_MODE)));
    // QUNS_NOT_PRESENT / ACCEPTS_NOTIFICATIONS / QUIET_TIME / APP.
    for raw in [1, 5, 6, 7] {
        assert!(!suppresses_toasts(Some(raw)), "raw {raw} must not suppress");
    }
    assert!(!suppresses_toasts(None));
}

#[test]
fn suppressed_via_source_trait() {
    assert!(meeting_toast_suppressed(&FixedSource(Some(QUNS_BUSY))));
    assert!(!meeting_toast_suppressed(&FixedSource(Some(1))));
    assert!(!meeting_toast_suppressed(&FixedSource(None)));
}

#[test]
fn visual_notch_rect_ignores_the_transparent_window_frame() {
    let rect = anchored_flowbar_rect(
        (1080.0, 1264.0, 400.0, 120.0),
        Some(crate::overlay::FlowbarRect {
            x: 176.0,
            y: 104.0,
            width: 48.0,
            height: 8.0,
        }),
        1.0,
    );
    assert_eq!(rect, (1256.0, 1368.0, 48.0, 8.0));
    let (_, y) = toast_origin(
        (0.0, 0.0, 2560.0, 1400.0),
        Some(rect),
        (376.0, 88.0),
        ToastPosition::AboveFlowbar,
        6.0,
        16.0,
    );
    assert_eq!(y, 1368.0 - 6.0 - 88.0);
}

#[test]
fn bottom_right_forces_corner() {
    let flowbar = Some((800.0, 900.0, 300.0, 100.0));
    let (x, y) = toast_origin(
        AREA,
        flowbar,
        (376.0, 88.0),
        ToastPosition::BottomRight,
        6.0,
        16.0,
    );
    assert_eq!((x, y), (1920.0 - 376.0 - 16.0, 1040.0 - 88.0 - 16.0));
}

#[test]
fn above_flowbar_centers_and_stacks() {
    let flowbar = Some((800.0, 900.0, 300.0, 100.0));
    let (x, y) = toast_origin(
        AREA,
        flowbar,
        (376.0, 88.0),
        ToastPosition::AboveFlowbar,
        6.0,
        16.0,
    );
    assert_eq!(x, 800.0 + 150.0 - 188.0); // centered on the bar
    assert_eq!(y, 900.0 - 6.0 - 88.0); // bottom edge `gap` above the bar top
}

#[test]
fn above_flowbar_falls_back_to_corner_without_bar() {
    let (x, y) = toast_origin(
        AREA,
        None,
        (376.0, 88.0),
        ToastPosition::AboveFlowbar,
        6.0,
        16.0,
    );
    assert_eq!((x, y), (1920.0 - 376.0 - 16.0, 1040.0 - 88.0 - 16.0));
}

#[test]
fn bar_docked_high_drops_toast_below_it() {
    // Flow Bar docked at the top edge: anchoring "above" would leave the
    // work area, so the toast drops just below the bar.
    let flowbar = Some((800.0, 4.0, 300.0, 40.0));
    let (x, y) = toast_origin(
        AREA,
        flowbar,
        (376.0, 88.0),
        ToastPosition::AboveFlowbar,
        6.0,
        16.0,
    );
    assert_eq!(x, 800.0 + 150.0 - 188.0);
    assert_eq!(y, 4.0 + 40.0 + 6.0);
}

#[test]
fn toast_clamped_inside_work_area() {
    // Bar hugging the right edge: centered-x would overflow the work area.
    let flowbar = Some((1800.0, 900.0, 120.0, 100.0));
    let (x, _) = toast_origin(
        AREA,
        flowbar,
        (376.0, 88.0),
        ToastPosition::AboveFlowbar,
        6.0,
        16.0,
    );
    assert_eq!(x, 1920.0 - 376.0 - 16.0);
}

#[test]
fn detector_payload_parses_new_and_ended() {
    let parsed: DetectorMeetingPayload = serde_json::from_str(
        r#"{"detection_id":"d1","app_label":"Google Meet","exe":"chrome.exe","action":"ask","started_at":123}"#,
    )
    .expect("new detection parses");
    assert!(!parsed.ended);
    assert_eq!(parsed.detection_id.as_deref(), Some("d1"));

    let ended: DetectorMeetingPayload =
        serde_json::from_str(r#"{"detection_id":"d1","ended":true}"#)
            .expect("ended detection parses");
    assert!(ended.ended);
    assert_eq!(ended.detection_id.as_deref(), Some("d1"));
}

#[test]
fn show_payload_parses_kind_message_action() {
    let parsed: ToastShowPayload = serde_json::from_str(
        r#"{"kind":"warning","message":"mic fallback","action":{"cmd":"open_settings"}}"#,
    )
    .expect("show payload parses");
    assert_eq!(parsed.kind.as_deref(), Some("warning"));
    assert_eq!(parsed.message.as_deref(), Some("mic fallback"));
    assert!(parsed.action.is_some());
}

#[test]
fn state_payload_always_serializes_all_keys() {
    let payload = ToastStatePayload {
        collapsed: true,
        detection: None,
        notice: None,
    };
    let json = serde_json::to_value(&payload).expect("payload serializes");
    assert_eq!(
        json,
        serde_json::json!({"collapsed": true, "detection": null, "notice": null})
    );
}
