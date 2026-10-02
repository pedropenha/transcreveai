//! Win32 icon extraction: shell image list (48 px) with `SHGFI_ICON` fallback,
//! then `GetIconInfo` + `GetDIBits` into straight RGBA.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX,
    SHIL_EXTRALARGE,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::{Extraction, IconExtractor, RgbaImage};

/// Largest icon accepted (guards the allocation below).
const MAX_ICON_DIM: i32 = 512;

pub(crate) struct WindowsIconExtractor;

/// Longest the caller waits for the shell. A stalled path (disconnected
/// mapped drive, AV hook) must not hold up the detector thread.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(2);

impl IconExtractor for WindowsIconExtractor {
    /// Runs the shell calls on a short-lived STA thread (the shell icon APIs
    /// need COM) and gives up after [`EXTRACT_TIMEOUT`]; a timed-out thread
    /// finishes on its own and its result is discarded.
    fn extract(&self, exe_path: &str) -> Extraction {
        let wide: Vec<u16> = exe_path.encode_utf16().chain(Some(0)).collect();
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("app-icon-extract".into())
            .spawn(move || {
                // SAFETY: plain COM init for this thread; paired with
                // `CoUninitialize` below only when it succeeded.
                let com_ready = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
                let result = extract_blocking(&wide);
                if com_ready {
                    // SAFETY: balances the successful `CoInitializeEx` above.
                    unsafe { CoUninitialize() };
                }
                let _ = tx.send(result);
            });
        if spawned.is_err() {
            return Extraction::Unavailable;
        }
        rx.recv_timeout(EXTRACT_TIMEOUT)
            .unwrap_or(Extraction::Unavailable)
    }
}

fn extract_blocking(wide: &[u16]) -> Extraction {
    let Some(icon) = OwnedIcon::from_image_list(wide).or_else(|| OwnedIcon::from_shell(wide))
    else {
        return Extraction::Unavailable;
    };
    icon_to_rgba(icon.0)
        .map(Extraction::Found)
        .unwrap_or(Extraction::Unavailable)
}

/// `HICON` destroyed on drop.
struct OwnedIcon(HICON);

impl OwnedIcon {
    /// 48 px icon via the system image list.
    fn from_image_list(wide: &[u16]) -> Option<Self> {
        let mut info = SHFILEINFOW::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `info` is a
        // valid out-pointer and `cbfileinfo` is its exact size.
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX,
            )
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: plain shell call with a valid list id; the returned COM
        // interface is released by the `windows` crate on drop.
        let list: IImageList = unsafe { SHGetImageList(SHIL_EXTRALARGE as i32) }.ok()?;
        // SAFETY: `list` is a live COM object; the returned HICON is owned by us.
        let hicon = unsafe { list.GetIcon(info.iIcon, ILD_TRANSPARENT.0) }.ok()?;
        (!hicon.is_invalid()).then_some(Self(hicon))
    }

    /// 32 px fallback via `SHGFI_ICON | SHGFI_LARGEICON`.
    fn from_shell(wide: &[u16]) -> Option<Self> {
        let mut info = SHFILEINFOW::default();
        // SAFETY: same contract as in `from_image_list`.
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            )
        };
        (ok != 0 && !info.hIcon.is_invalid()).then_some(Self(info.hIcon))
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: the handle was created by the shell for us and is destroyed
        // exactly once, here.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

/// GDI bitmap deleted on drop (null handles are ignored).
struct OwnedBitmap(HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `GetIconInfo` handed us ownership of this bitmap.
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0 .0)) };
        }
    }
}

/// Screen DC released on drop.
struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: obtained from `GetDC(None)` and released once.
        unsafe { ReleaseDC(None::<HWND>, self.0) };
    }
}

fn icon_to_rgba(icon: HICON) -> Option<RgbaImage> {
    let mut info = ICONINFO::default();
    // SAFETY: `icon` is valid for the call and `info` is a valid out-pointer.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let color = OwnedBitmap(info.hbmColor);
    let mask = OwnedBitmap(info.hbmMask);
    if color.0.is_invalid() {
        return None; // monochrome icon: not worth a mask-only rendering
    }
    let (width, height) = bitmap_size(color.0)?;
    // SAFETY: a null window handle asks for the whole-screen DC.
    let dc = ScreenDc(unsafe { GetDC(None) });
    let mut bgra = read_bgra(dc.0, color.0, width, height)?;
    if bgra.as_chunks::<4>().0.iter().all(|px| px[3] == 0) {
        // Pre-alpha icon: transparency lives in the AND mask (0 = opaque).
        let mask_px = read_bgra(dc.0, mask.0, width, height)?;
        for (px, m) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask_px.as_chunks::<4>().0)
        {
            px[3] = if m[0] == 0 { 255 } else { 0 };
        }
    }
    bgra.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .for_each(|px| px.swap(0, 2));
    Some(RgbaImage {
        width: width as u32,
        height: height as u32,
        rgba: bgra,
    })
}

fn bitmap_size(bitmap: HBITMAP) -> Option<(i32, i32)> {
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a valid out-buffer of exactly `size_of::<BITMAP>()`.
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        )
    };
    let valid = |d: i32| (1..=MAX_ICON_DIM).contains(&d);
    (got != 0 && valid(bm.bmWidth) && valid(bm.bmHeight)).then_some((bm.bmWidth, bm.bmHeight))
}

/// Read `bitmap` as top-down 32-bit BGRA.
fn read_bgra(dc: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative = top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: `buf` holds exactly `height` rows of `width` 32-bit pixels as
    // declared in `bmi`; `dc`/`bitmap` are live handles.
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    (lines == height).then_some(buf)
}
