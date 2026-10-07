//! Writing aids shared by the main editor and quick capture: @-mention
//! autocomplete and spellcheck. The pure logic lives in `mention` and
//! `spell`; `assist` is the GPUI part a host view embeds.

pub mod assist;
pub mod mention;
pub mod picker;
pub mod spell;
#[cfg(target_os = "macos")]
pub mod spell_mac;
pub mod text;

use std::sync::Arc;

use gpui_kit::{App, Global};

pub use assist::{Assist, AssistKey};
use spell::{Cache, SpellEngine};

gpui_kit::actions!(blygger, [ToggleSpellcheck]);

/// The spell checker, whether it's on (`spellcheck`), and the results cache
/// every editor shares.
pub struct SpellService {
    pub engine: Option<Arc<dyn SpellEngine>>,
    pub enabled: bool,
    pub cache: Cache,
}

impl Global for SpellService {}

/// Install the spell service. `engine` is `None` where there's no system
/// checker (spellcheck then does nothing).
pub fn init(engine: Option<Arc<dyn SpellEngine>>, enabled: bool, cx: &mut App) {
    cx.set_global(SpellService {
        engine,
        enabled,
        cache: Cache::default(),
    });
}

/// The system engine for this platform.
pub fn system_engine() -> Option<Arc<dyn SpellEngine>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(spell_mac::MacSpell::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub fn spellcheck_enabled(cx: &App) -> bool {
    cx.try_global::<SpellService>().is_some_and(|s| s.enabled)
}

/// Turn checking on or off everywhere (the config file is the caller's).
pub fn set_spellcheck(on: bool, cx: &mut App) {
    if let Some(s) = cx.try_global::<SpellService>()
        && s.enabled == on
    {
        return;
    }
    if !cx.has_global::<SpellService>() {
        init(None, on, cx);
    }
    cx.global_mut::<SpellService>().enabled = on;
}

/// Edit › Spelling › Check Spelling While Typing: flip it, remember it in
/// the config file, and re-check the menu item.
pub fn toggle_spellcheck(cx: &mut App) -> Result<bool, String> {
    let on = !spellcheck_enabled(cx);
    set_spellcheck(on, cx);
    crate::refresh_menus(cx);
    let change = blyg_core::config::edit::Change::Set(on.to_string());
    crate::settings::write(&[("spellcheck", change)], cx).map(|()| on)
}
