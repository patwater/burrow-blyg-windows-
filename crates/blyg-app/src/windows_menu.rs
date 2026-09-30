//! Windows has no global menu bar, so the menus `main::menus` gives macOS
//! would be out of reach there, and with them the actions that have no key
//! (Subscribe…, Site Settings…, Open Config File…). This adds a "Menu"
//! button to the title strip, in the space macOS keeps for the traffic
//! lights, that opens every item of those same menus (read back with
//! `get_menus`) with the key each one is bound to.
//!
//! Windows only; `app.rs` calls it through `crate::platform`.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::TITLEBAR_H;
use crate::theme::Palette;

/// Whether the menu is open (one per app: there is one main window).
#[derive(Default)]
struct MenuOpen(bool);

impl Global for MenuOpen {}

fn is_open(cx: &App) -> bool {
    cx.try_global::<MenuOpen>().is_some_and(|m| m.0)
}

fn set_open(open: bool, window: &mut Window, cx: &mut App) {
    cx.set_global(MenuOpen(open));
    window.refresh();
}

/// The "Menu" button, over the left end of the title strip.
pub fn button(p: Palette, cx: &App) -> AnyElement {
    let open = is_open(cx);
    div()
        .id("windows-menu-button")
        .absolute()
        .top_0()
        .left(px(8.))
        .h(px(TITLEBAR_H))
        .flex()
        .items_center()
        .child(
            div()
                .id("windows-menu-label")
                .px(px(9.))
                .py(px(3.))
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(p.ink)
                .when(open, |d| d.bg(p.sel))
                .hover(|s| s.bg(p.sel))
                .cursor_pointer()
                .child("Menu")
                .on_click(|_, window, cx| set_open(!is_open(cx), window, cx)),
        )
        .into_any_element()
}

/// The open menu: every menu's items under its name, with their keys.
pub fn panel(p: Palette, window: &Window, cx: &App) -> Option<AnyElement> {
    if !is_open(cx) {
        return None;
    }
    let menus = cx.get_menus().unwrap_or_default();
    let max_h = (window.viewport_size().height - px(TITLEBAR_H + 16.)).max(px(120.));
    let mut list = div()
        .id("windows-menu-list")
        .flex()
        .flex_col()
        .max_h(max_h)
        .overflow_y_scroll()
        .py(px(4.));
    let mut n = 0;
    for menu in &menus {
        list = add_menu(list, menu, p, window, &mut n);
    }
    let panel = div()
        .absolute()
        .top(px(TITLEBAR_H))
        .left(px(8.))
        .w(px(300.))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .rounded(px(6.))
        .shadow_lg()
        .occlude()
        .text_size(px(12.5))
        .on_mouse_down_out(|_, window, cx| set_open(false, window, cx))
        .child(list);
    Some(deferred(panel).with_priority(2).into_any_element())
}

fn add_menu(
    mut list: Stateful<Div>,
    menu: &OwnedMenu,
    p: Palette,
    window: &Window,
    n: &mut usize,
) -> Stateful<Div> {
    list = list.child(
        div()
            .px(px(12.))
            .pt(px(8.))
            .pb(px(2.))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.muted)
            .child(menu.name.clone()),
    );
    for item in &menu.items {
        match item {
            OwnedMenuItem::Separator => {
                list = list.child(div().mx(px(12.)).my(px(4.)).h(px(1.)).bg(p.line));
            }
            OwnedMenuItem::Submenu(sub) => list = add_menu(list, sub, p, window, n),
            OwnedMenuItem::SystemMenu(_) => {}
            OwnedMenuItem::Action {
                name,
                action,
                disabled,
                ..
            } => {
                *n += 1;
                let key = window
                    .highest_precedence_binding_for_action(&**action)
                    .and_then(|b| {
                        b.keystrokes()
                            .first()
                            .map(|k| crate::keymap::glyphs(&k.unparse()))
                    })
                    .unwrap_or_default();
                let run = action.boxed_clone();
                let disabled = *disabled;
                list = list.child(
                    div()
                        .id(("windows-menu-item", *n))
                        .mx(px(4.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(4.))
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .text_color(if disabled { p.muted } else { p.ink })
                        .when(!disabled, |d| {
                            d.cursor_pointer().hover(|s| s.bg(p.sel)).on_click(
                                move |_, window, cx| {
                                    set_open(false, window, cx);
                                    window.dispatch_action(run.boxed_clone(), cx);
                                },
                            )
                        })
                        .child(name.clone())
                        .child(div().text_color(p.muted).child(key)),
                );
            }
        }
    }
    list
}
