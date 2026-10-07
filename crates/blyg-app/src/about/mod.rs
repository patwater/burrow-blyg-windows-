//! Burrow › About Burrow (also Help › About Burrow): a small window with
//! the version and build, the updater's state, the connection (host and
//! detected server capabilities, never the token), the data and config
//! paths, links, and "Copy build info" for bug reports.
//!
//! `info` holds the data and the copied text; this file is the window.

pub mod info;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

use blyg_core::ConfigStore;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::connection::{self, Mode};
use crate::prefs::Prefs;
use crate::theme::Rule;
use info::{BuildInfo, ConnectionInfo, Report, UpdateInfo}; // --- themes --- dividers

gpui_kit::actions!(blygger, [ShowAbout]);

/// `packaging/icon.svg` at 512 px, the same bytes as the Dock icon.
static ICON_PNG: &[u8] = include_bytes!("../../../../packaging/icon-512.png");

const W: f32 = 460.;
const H: f32 = 770.;

/// The open About window, if any (there's only ever one).
#[derive(Default)]
pub struct AboutGlobal {
    pub window: Option<WindowHandle<AboutView>>,
}

impl Global for AboutGlobal {}

/// Register the action (both menu items dispatch it).
pub fn init(cx: &mut App) {
    cx.set_global(AboutGlobal::default());
    // Deferred: dispatched from inside the About window itself, that window
    // is busy and couldn't be brought forward (a second one would open).
    cx.on_action(|_: &ShowAbout, cx| cx.defer(open));
}

/// The About window, if it's open.
pub fn window(cx: &App) -> Option<WindowHandle<AboutView>> {
    cx.try_global::<AboutGlobal>().and_then(|g| g.window)
}

/// Open the About window, or bring the open one forward.
pub fn open(cx: &mut App) {
    if let Some(h) = window(cx)
        && h.update(cx, |_, window, _| activate(window)).is_ok()
    {
        return;
    }
    let prefs = prefs(cx);
    crate::fonts::ensure(prefs.ui().bundled, cx);
    crate::fonts::ensure(prefs.writing().bundled, cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(W), px(H)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("About Burrow".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(11.))),
        }),
        focus: !crate::no_activate(),
        is_resizable: false,
        is_minimizable: false,
        app_id: Some(blyg_core::config::APP_ID.into()),
        ..Default::default()
    };
    match cx.open_window(opts, |window, cx| cx.new(|cx| AboutView::new(window, cx))) {
        Ok(h) => {
            let _ = h.update(cx, |_, window, _| activate(window));
            cx.default_global::<AboutGlobal>().window = Some(h);
        }
        Err(e) => eprintln!("blygger: couldn't open the About window: {e}"),
    }
}

fn activate(window: &mut Window) {
    if crate::no_activate() {
        crate::order_front_regardless(window);
    } else {
        window.activate_window();
    }
}

fn prefs(cx: &App) -> Prefs {
    let mut prefs = match cx.try_global::<crate::settings::AppConfig>() {
        Some(c) => Prefs::from_config(c.store.config()),
        None => Prefs::from_config(ConfigStore::in_memory("").config()),
    };
    prefs.apply_theme_override();
    prefs
}

fn data_dir(cx: &App) -> PathBuf {
    cx.try_global::<connection::Connection>()
        .map(|c| c.data_dir.clone())
        .unwrap_or_else(blyg_core::config::data_dir)
}

fn config_path(cx: &App) -> Option<PathBuf> {
    cx.try_global::<crate::settings::AppConfig>()
        .map(|c| c.store.primary().to_path_buf())
}

/// What the app knows about the server it's connected to.
fn connection_info(cx: &App) -> ConnectionInfo {
    let Some(c) = cx.try_global::<connection::Connection>() else {
        return ConnectionInfo::Disconnected;
    };
    let b = &c.switch;
    let caps = || {
        use blyg_core::Backend as _;
        info::capabilities(
            b.server_extensions()
                .unwrap_or_else(|| b.read_extensions_available()),
            b.read_state_sync(),
            b.provenance_available(),
        )
    };
    match c.switch.mode() {
        Mode::Disconnected => ConnectionInfo::Disconnected,
        Mode::Fake => ConnectionInfo::Sample { caps: caps() },
        Mode::Live => {
            let host = cx
                .try_global::<crate::settings::AppConfig>()
                .and_then(|a| a.store.config().blyg_url().and_then(crate::vm::url_host))
                .unwrap_or_else(|| "unknown".into());
            ConnectionInfo::Live { host, caps: caps() }
        }
    }
}

/// Everything the window shows (except the paths).
pub fn report(cx: &App) -> Report {
    let last_check = blyg_core::state::AppState::load(&data_dir(cx)).last_update_check;
    Report {
        build: BuildInfo::current(),
        macos: macos_version().to_string(),
        update: UpdateInfo {
            auto_update: prefs(cx).auto_update,
            last_check,
            status: crate::update::status_text(cx),
        },
        connection: connection_info(cx),
    }
}

/// The OS name and version for the report. Elsewhere than macOS: the OS name.
#[cfg(not(target_os = "macos"))]
pub fn macos_version() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else {
        std::env::consts::OS
    }
}

/// `15.6.1 (Build 24G90)`, from NSProcessInfo.
#[cfg(target_os = "macos")]
pub fn macos_version() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        // SAFETY: NSProcessInfo is thread-safe; nil is checked at each step
        // and the UTF-8 buffer is copied before the autoreleased string goes.
        let s = unsafe {
            let pi: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            if pi.is_null() {
                return "unknown".into();
            }
            let s: *mut AnyObject = msg_send![pi, operatingSystemVersionString];
            if s.is_null() {
                return "unknown".into();
            }
            let p: *const std::ffi::c_char = msg_send![s, UTF8String];
            if p.is_null() {
                return "unknown".into();
            }
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        };
        s.strip_prefix("Version ").unwrap_or(&s).to_string()
    })
}

/// Finder (Explorer on Windows), with `path` selected (or its folder, if it
/// doesn't exist yet).
fn reveal(path: &Path) {
    if cfg!(target_os = "windows") {
        let mut cmd = std::process::Command::new("explorer.exe");
        if path.exists() {
            // One argument: Explorer parses `/select,` and the path itself.
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path);
            cmd.arg(arg);
        } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
            cmd.arg(parent);
        } else {
            return;
        }
        if let Err(e) = cmd.spawn() {
            eprintln!("blygger: couldn't open Explorer: {e}");
        }
        return;
    }
    let mut cmd = std::process::Command::new("/usr/bin/open");
    if path.exists() {
        cmd.arg("-R").arg(path);
    } else if let Some(parent) = path.parent().filter(|p| p.exists()) {
        cmd.arg(parent);
    } else {
        return;
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("blygger: couldn't open Finder: {e}");
    }
}

pub struct AboutView {
    focus: FocusHandle,
    /// "Copied" shows on the button for a moment after a copy.
    copied: bool,
}

impl AboutView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // Follow the system appearance when theme = system.
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        AboutView {
            focus,
            copied: false,
        }
    }

    pub fn copy_build_info(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(report(cx).copy_text()));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1600))
                .await;
            let _ = this.update(cx, |v, cx| {
                v.copied = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for AboutView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut prefs = prefs(cx);
        // --- themes ---
        let theme = crate::theme::resolve(&prefs, window.appearance(), cx);
        prefs.adopt_theme_fonts(&theme);
        crate::fonts::ensure(prefs.ui().bundled, cx);
        crate::fonts::ensure(prefs.writing().bundled, cx);
        let p = theme.palette;
        let ui_font: SharedString = prefs.ui().family.into();
        let serif: SharedString = prefs.writing().family.into();
        let r = report(cx);
        let b = &r.build;

        let label = |s: &'static str| {
            div()
                .w(px(104.))
                .flex_none()
                .pt(px(1.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.muted)
                .child(s)
        };
        let row = |l: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .py(px(2.5))
                .child(label(l))
                .child(div().flex_1().min_w_0().child(value))
        };
        let text = |s: String| div().child(s).into_any_element();
        let section = |title: &'static str| {
            div()
                .mt(px(12.))
                .mb(px(4.))
                .pb(px(3.))
                .rule_b(&p)
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(crate::theme_ext::readable(
                    p.accent,
                    p.panel(),
                    crate::theme_ext::TEXT,
                ))
                .child(title)
        };
        let link = |id: &'static str, s: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .cursor_pointer()
                .text_color(crate::theme_ext::readable(
                    p.accent,
                    p.panel(),
                    crate::theme_ext::TEXT,
                ))
                .hover(|s| s.underline())
                .child(s)
        };
        let button = |id: &'static str, s: SharedString| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px(px(10.))
                .py(px(3.))
                .flex_none()
                .rounded(px(5.))
                .border_1()
                .border_color(p.edge())
                .bg(p.panel())
                .cursor_pointer()
                .hover(move |s| s.border_color(p.accent).text_color(p.accent_text()))
                .child(s)
        };
        let path_row = |l: &'static str, id: &'static str, path: Option<PathBuf>| {
            let shown = path
                .as_deref()
                .map(blyg_core::config::paths::tilde)
                .unwrap_or_else(|| "unknown".into());
            // The path wraps in the value column; the link sits under the
            // label, so a long path doesn't squeeze it.
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .py(px(2.5))
                .child(
                    div()
                        .w(px(104.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(label(l))
                        .when_some(path, |d, path| {
                            d.child(
                                link(id, "Reveal in Finder")
                                    .text_size(px(11.))
                                    .on_click(move |_, _, _| reveal(&path)),
                            )
                        }),
                )
                .child(div().flex_1().min_w_0().child(shown))
        };

        let header = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(
                img(std::sync::Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    ICON_PNG.to_vec(),
                )))
                .size(px(72.))
                .flex_none(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_family(serif.clone())
                            .text_size(px(28.))
                            .child("Burrow"),
                    )
                    .child(div().text_color(p.muted).child(format!(
                        "Version {} ({})",
                        b.version,
                        b.commit()
                    )))
                    .child(
                        div()
                            .font_family(serif)
                            .italic()
                            .text_color(p.muted)
                            .child("Burrow is a blygger client."),
                    ),
            );

        let caps = r.connection.caps().iter().map(|c| {
            row(
                c.label,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .when(c.on, |d| {
                                d.bg(crate::theme_ext::readable(
                                    p.green,
                                    p.bg,
                                    crate::theme_ext::MARK,
                                ))
                            })
                            .when(!c.on, |d| d.border_1().border_color(p.grey)),
                    )
                    .child(if c.on { "yes" } else { "no" })
                    .into_any_element(),
            )
        });

        let version = b.version;
        div()
            .id("about")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                if k.key == "escape" || (k.modifiers.platform && k.key == "w") {
                    cx.stop_propagation();
                    window.remove_window();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.ink)
            .font_family(ui_font)
            .text_size(px(12.5))
            // The transparent title bar: a drag area the height of the
            // main window's.
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .id("about-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .pb(px(12.))
                    .child(header)
                    .child(section("BUILD"))
                    .child(row("Commit", text(b.commit())))
                    .child(row("Built", text(b.date())))
                    .child(row("Profile", text(b.profile.to_string())))
                    .child(row("Architecture", text(b.arch_line())))
                    .child(row("macOS", text(r.macos.clone())))
                    .child(section("UPDATES"))
                    .child(row(
                        "Auto-update",
                        text(info::auto_update_value(r.update.auto_update).to_string()),
                    ))
                    .child(row("Last check", text(r.update.last_check_text())))
                    .child(row("Status", text(r.update.status.clone())))
                    .child(
                        div().pl(px(114.)).pt(px(4.)).flex().child(
                            button("about-check", "Check for Updates…".into())
                                .on_click(|_, _, cx| cx.defer(crate::update::check_now)),
                        ),
                    )
                    .child(section("CONNECTION"))
                    .child(row("Blyg", text(r.connection.summary())))
                    .children(caps)
                    // A stock server: what that limits (as the sheet said).
                    .when(r.connection.limited(), |d| {
                        d.child(
                            div()
                                .pl(px(114.))
                                .pt(px(4.))
                                .text_color(p.muted)
                                .line_height(relative(1.4))
                                .child("Stock blygger-studio. Limited here:")
                                .children(
                                    crate::app::server_notice::LIMITS
                                        .iter()
                                        .map(|l| div().mt(px(3.)).child(format!("• {l}"))),
                                ),
                        )
                    })
                    .child(section("FILES"))
                    .child(path_row("Data", "about-reveal-data", Some(data_dir(cx))))
                    .child(path_row("Config", "about-reveal-config", config_path(cx))),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(28.))
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .bg(p.panel())
                    .rule_t(&p)
                    .child(link("about-github", "GitHub").on_click(|_, _, cx| {
                        cx.open_url(&info::repo_url());
                    }))
                    .child(div().text_color(p.muted).child("·"))
                    .child(
                        link("about-release", "Release notes").on_click(move |_, _, cx| {
                            cx.open_url(&info::release_url(version));
                        }),
                    )
                    .child(div().text_color(p.muted).child("·"))
                    .child(link("about-license", "License").on_click(|_, _, cx| {
                        cx.open_url(&info::license_url());
                    }))
                    .child(div().flex_1())
                    .child(
                        button(
                            "about-copy",
                            if self.copied {
                                "Copied".into()
                            } else {
                                "Copy build info".into()
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy_build_info(cx))),
                    ),
            )
    }
}

/// `BLYGGER_DEMO=about`: open the window; with `BLYGGER_SNAPSHOT`, save its
/// frame (in a `blygger_snap` build; see reading/demo.rs) and quit.
pub fn demo(step: usize, cx: &mut App) {
    match step {
        0 => open(cx),
        2 => {
            let (Some(h), Ok(path)) = (window(cx), std::env::var("BLYGGER_SNAPSHOT")) else {
                return;
            };
            let _ = h.update(cx, |_, window, cx| {
                cx.spawn_in(window, async move |_, cx| {
                    for _ in 0..3 {
                        let _ = cx.update(|window, cx| window.draw(cx).clear(cx));
                        cx.background_executor()
                            .timer(Duration::from_millis(300))
                            .await;
                    }
                    let _ = cx.update(|window, cx| {
                        window.draw(cx).clear(cx);
                        crate::app::reading::demo::snap::save_frame(window, &path);
                        cx.quit();
                    });
                })
                .detach();
            });
        }
        _ => {}
    }
}
