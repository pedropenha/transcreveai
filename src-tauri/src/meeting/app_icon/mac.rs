//! macOS icon extraction: `NSWorkspace.iconForFile` returns an `NSImage` for
//! a `.app` bundle (or any file); it is drawn into a fixed-size
//! `NSBitmapImageRep` and PNG-encoded. `iconForFile` falls back to the
//! generic document icon for anything without a bundle icon, so
//! `Unavailable` is only emitted when allocation or encoding itself fails.
//!
//! Runs inside `spawn_blocking`: `NSWorkspace`/`NSImage` plumbing here has
//! no AppKit main-thread requirement.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

use super::{Extraction, IconExtractor};

/// Rendered icon edge in px — covers the largest history tile (48 px) at
/// Retina scale and keeps PNGs around 5–15 KB, far under MAX_DATA_URI_BYTES.
const ICON_PX: isize = 64;

pub(crate) struct MacIconExtractor;

impl IconExtractor for MacIconExtractor {
    fn extract(&self, path: &str) -> Extraction {
        let path = NSString::from_str(path);
        let image = NSWorkspace::sharedWorkspace().iconForFile(&path);

        // SAFETY: null planes → AppKit allocates the backing store; the
        // geometry below (RGBA/non-planar, 8 bps) matches a 32-bit pixel.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                ICON_PX,
                ICON_PX,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                32,
            )
        };
        let Some(rep) = rep else {
            return Extraction::Unavailable;
        };
        let Some(ctx) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) else {
            return Extraction::Unavailable;
        };
        let previous = NSGraphicsContext::currentContext();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        image.drawInRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(ICON_PX as f64, ICON_PX as f64),
        ));
        NSGraphicsContext::setCurrentContext(previous.as_deref());
        ctx.flushGraphics();

        let props: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
        // SAFETY: PNG encoding accepts an empty property dictionary.
        match unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        {
            Some(data) => Extraction::FoundPng(data.to_vec()),
            None => Extraction::Unavailable,
        }
    }
}
