//! The assistant uses native placement; hiding and clipping must use the same HWND.

use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
use ::windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE};

const FRAMELESS_SUBCLASS: usize = 0x415349;

fn caption_mask() -> isize {
    use ::windows::Win32::UI::WindowsAndMessaging::{
        WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };
    (WS_CAPTION.0 | WS_SYSMENU.0 | WS_THICKFRAME.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0) as isize
}

// Installed on the HWND's owning UI thread. No allocated callback state.
unsafe extern "system" fn frameless_proc(
    hwnd: HWND,
    message: u32,
    wparam: ::windows::Win32::Foundation::WPARAM,
    lparam: ::windows::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> ::windows::Win32::Foundation::LRESULT {
    use ::windows::Win32::Foundation::{LPARAM, LRESULT};
    use ::windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use ::windows::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, STYLESTRUCT, WM_NCACTIVATE, WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING,
    };
    match message {
        WM_STYLECHANGING if wparam.0 as i32 == GWL_STYLE.0 && lparam.0 != 0 => {
            // Tao's cached flags include WS_CAPTION even for an undecorated window.
            let styles = &mut *(lparam.0 as *mut STYLESTRUCT);
            styles.styleNew &= !(caption_mask() as u32);
        }
        WM_NCPAINT => return LRESULT(0),
        WM_NCACTIVATE => {
            // Preserve Tao's focus handling, but tell DefWindowProc not to repaint
            // the non-client area behind the translucent WebView.
            return DefSubclassProc(hwnd, message, wparam, LPARAM(-1));
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(frameless_proc), FRAMELESS_SUBCLASS);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// Tauri's undecorated transparent HWND can retain a caption behind the WebView.
pub(in crate::assistant) fn frameless(hwnd: HWND) -> ::windows::core::Result<()> {
    use ::windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY,
    };
    use ::windows::Win32::UI::Shell::SetWindowSubclass;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };
    unsafe {
        if !SetWindowSubclass(hwnd, Some(frameless_proc), FRAMELESS_SUBCLASS, 0).as_bool() {
            return Err(::windows::core::Error::from_win32());
        }
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            (&DWMNCRP_DISABLED as *const ::windows::Win32::Graphics::Dwm::DWMNCRENDERINGPOLICY)
                .cast(),
            std::mem::size_of_val(&DWMNCRP_DISABLED) as u32,
        )?;
        let mask = caption_mask();
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style & mask != 0 {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !mask);
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            )?;
            if GetWindowLongPtrW(hwnd, GWL_STYLE) & mask != 0 {
                return Err(::windows::core::Error::from_win32());
            }
        }
    }
    Ok(())
}

pub(in crate::assistant) fn hide(hwnd: HWND) -> ::windows::core::Result<()> {
    // ShowWindow's result means "was visible", not success. Check the actual HWND.
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        if IsWindowVisible(hwnd).as_bool() {
            return Err(::windows::core::Error::from_win32());
        }
    }
    Ok(())
}

pub(in crate::assistant) fn clip(
    hwnd: HWND,
    width: i32,
    height: i32,
    scale: f64,
) -> ::windows::core::Result<()> {
    let diameter = (48.0 * scale).round().max(1.0) as i32;
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.0.is_null() {
            return Err(::windows::core::Error::from_win32());
        }
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            // Ownership transfers to Windows only on success.
            let _ = DeleteObject(region.into());
            return Err(::windows::core::Error::from_win32());
        }
    }
    Ok(())
}

pub(in crate::assistant) fn clip_window(window: &tauri::WebviewWindow) {
    let result = (|| {
        let hwnd = window.hwnd()?;
        frameless(hwnd)?;
        let size = window.outer_size()?;
        let scale = window.scale_factor()? * crate::overlay::windows_text_scale_factor();
        clip(hwnd, size.width as i32, size.height as i32, scale)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        log::warn!("Assistant rounded clipping unavailable: {error}");
    }
}

#[cfg(test)]
mod tests {
    use ::windows::core::w;
    use ::windows::Win32::Foundation::HWND;
    use ::windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, GetWindowRgn, PtInRegion};
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW, WINDOW_EX_STYLE, WS_POPUP,
    };

    struct TestWindow(HWND);
    impl TestWindow {
        fn new() -> Self {
            // A window owned by this test thread; never activates or captures audio.
            Self(unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Assistant test"),
                    WS_POPUP,
                    0,
                    0,
                    440,
                    640,
                    None,
                    None,
                    None,
                    None,
                )
                .expect("test window")
            })
        }
    }
    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn removes_native_caption_and_system_buttons() {
        use ::windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, WS_CAPTION, WS_SYSMENU,
        };
        let window = TestWindow::new();
        unsafe {
            let style = GetWindowLongPtrW(window.0, GWL_STYLE);
            SetWindowLongPtrW(
                window.0,
                GWL_STYLE,
                style | (WS_CAPTION.0 | WS_SYSMENU.0) as isize,
            );
            assert_ne!(
                GetWindowLongPtrW(window.0, GWL_STYLE) & WS_CAPTION.0 as isize,
                0
            );
        }
        super::frameless(window.0).expect("remove caption");
        unsafe {
            assert_eq!(
                GetWindowLongPtrW(window.0, GWL_STYLE) & (WS_CAPTION.0 | WS_SYSMENU.0) as isize,
                0
            );
        }
    }

    #[test]
    fn stays_frameless_when_window_styles_are_reapplied() {
        use ::windows::core::BOOL;
        use ::windows::Win32::Foundation::{LPARAM, WPARAM};
        use ::windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_NCRENDERING_ENABLED};
        use ::windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SendMessageW, SetWindowLongPtrW, GWL_STYLE, WM_NCACTIVATE,
            WM_NCPAINT, WS_CAPTION, WS_SYSMENU,
        };
        let window = TestWindow::new();
        super::frameless(window.0).expect("prepare frameless window");
        // Tao recomputes the native style from its cached flags on state changes.
        for _ in 0..3 {
            unsafe {
                let style = GetWindowLongPtrW(window.0, GWL_STYLE);
                SetWindowLongPtrW(
                    window.0,
                    GWL_STYLE,
                    style | (WS_CAPTION.0 | WS_SYSMENU.0) as isize,
                );
                SetWindowPos(window.0, None, 10, 20, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE)
                    .expect("move window");
                SendMessageW(window.0, WM_NCPAINT, Some(WPARAM(1)), Some(LPARAM(0)));
                SendMessageW(window.0, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(0)));
                assert_eq!(
                    GetWindowLongPtrW(window.0, GWL_STYLE) & (WS_CAPTION.0 | WS_SYSMENU.0) as isize,
                    0
                );
                let mut enabled = BOOL::default();
                DwmGetWindowAttribute(
                    window.0,
                    DWMWA_NCRENDERING_ENABLED,
                    (&mut enabled as *mut BOOL).cast(),
                    std::mem::size_of::<BOOL>() as u32,
                )
                .expect("read native rendering state");
                assert!(!enabled.as_bool());
            }
        }
    }

    #[test]
    fn closes_a_window_shown_through_native_placement() {
        let window = TestWindow::new();
        unsafe {
            SetWindowPos(
                window.0,
                None,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            )
            .expect("show");
            assert!(IsWindowVisible(window.0).as_bool());
        }
        super::hide(window.0).expect("hide");
        assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
    }

    #[test]
    fn rounded_region_removes_acrylic_corners_at_each_scale() {
        let window = TestWindow::new();
        for scale in [1.0, 1.5, 2.0] {
            super::clip(window.0, 440, 640, scale).expect("clip");
            unsafe {
                let region = CreateRectRgn(0, 0, 0, 0);
                assert_ne!(GetWindowRgn(window.0, region).0, 0);
                assert!(!PtInRegion(region, 0, 0).as_bool());
                assert!(PtInRegion(region, 220, 320).as_bool());
                assert!(PtInRegion(region, 220, 0).as_bool());
                let _ = DeleteObject(region.into());
            }
        }
    }
}
