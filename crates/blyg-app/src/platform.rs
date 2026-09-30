//! Small platform touches GPUI doesn't cover. Most are macOS-only; on
//! Windows the icon comes from the executable's resources (`build.rs`).

/// `packaging/icon.svg` rendered at 512 px (`scripts/make-icon.sh`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
static ICON_PNG: &[u8] = include_bytes!("../../../packaging/icon-512.png");

/// Whether this process runs from inside an `.app` bundle.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_bundled(exe: &std::path::Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// A bare `cargo run` binary has no bundle, so the Dock shows a generic
/// icon. Give it ours. Launched from Blygger.app, the bundle's .icns is
/// already in use and this does nothing.
#[cfg(target_os = "macos")]
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

/// Windows has no menu bar: a "Menu" button in the title strip instead
/// (`windows_menu`). Nothing elsewhere.
pub fn menu_button(p: crate::theme::Palette, cx: &gpui_kit::App) -> Option<gpui_kit::AnyElement> {
    #[cfg(target_os = "windows")]
    return Some(crate::windows_menu::button(p, cx));
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (p, cx);
        None
    }
}

/// The open Windows menu, if any.
pub fn menu_panel(
    p: crate::theme::Palette,
    window: &gpui_kit::Window,
    cx: &gpui_kit::App,
) -> Option<gpui_kit::AnyElement> {
    #[cfg(target_os = "windows")]
    return crate::windows_menu::panel(p, window, cx);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (p, window, cx);
        None
    }
}

/// Off macOS the taskbar and window icon is embedded in the executable.
#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon_unless_bundled() {}

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
