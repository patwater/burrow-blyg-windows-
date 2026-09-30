//! Small macOS-only touches GPUI doesn't cover.

/// `packaging/icon.svg` rendered at 512 px (`scripts/make-icon.sh`).
static ICON_PNG: &[u8] = include_bytes!("../../../packaging/icon-512.png");

/// Whether this process runs from inside an `.app` bundle.
pub fn is_bundled(exe: &std::path::Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// A bare `cargo run` binary has no bundle, so the Dock shows a generic
/// icon. Give it ours. Launched from Blygger.app, the bundle's .icns is
/// already in use and this does nothing.
pub fn set_dock_icon_unless_bundled() {
    if std::env::current_exe().is_ok_and(|p| is_bundled(&p)) {
        return;
    }
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: plain AppKit calls on the main thread (GPUI's run callback),
    // with nil checks; the NSData copies the static bytes.
    unsafe {
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICON_PNG.as_ptr() as *const std::ffi::c_void,
            length: ICON_PNG.len()
        ];
        if data.is_null() {
            return;
        }
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !app.is_null() {
            let _: () = msg_send![app, setApplicationIconImage: image];
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_an_embedded_png() {
        assert!(super::ICON_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(super::is_bundled(std::path::Path::new(
            "/Applications/Blygger.app/Contents/MacOS/blygger"
        )));
        assert!(!super::is_bundled(std::path::Path::new(
            "/tmp/target/release/blygger"
        )));
    }
}
