//! The Ghostty-style config file: parser, includes, write-back, locations,
//! migration from the old TOML files, and `+show-config` output.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use blyg_core::config::edit::{self, Change};
use blyg_core::config::keys::KEYS;
use blyg_core::config::migrate::migrate_legacy_files;
use blyg_core::config::parse::{self, Disk};
use blyg_core::config::paths::{ConfigFiles, data_dir_with, migrate_data_dir};
use blyg_core::config::show::{ShowOptions, show_config};
use blyg_core::config::{ConfigStore, Layout, Severity, Theme};
use blyg_core::state::AppState;

fn load_text(text: &str) -> blyg_core::config::Loaded {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config");
    std::fs::write(&p, text).unwrap();
    parse::load(&[p], &Disk)
}

#[test]
fn comments_blank_lines_and_quotes() {
    let l = load_text(
        "# a comment\n\
         \n\
         \x20 # indented comment\n\
         theme = dark\n\
         font-family-writing = \"Source Serif 4\"\n\
         ai-style-prompt = \"  keep my spaces \\\"quoted\\\" \\n next  \"\n\
         cloudflare-account-id = #not-a-comment\n\
         font-size=21\n",
    );
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    let c = &l.config;
    assert_eq!(c.theme(), Theme::Dark);
    assert_eq!(c.font_family_writing(), "Source Serif 4");
    assert_eq!(
        c.ai_style_prompt(),
        Some("  keep my spaces \"quoted\" \n next  ")
    );
    assert_eq!(c.cloudflare_account_id(), Some("#not-a-comment"));
    assert_eq!(c.font_size(), 21.0);
    // Untouched keys keep their defaults.
    assert_eq!(c.layout(), Layout::Side);
    assert_eq!(c.capture_hotkey(), "ctrl+alt+b");
    assert_eq!(c.blyg_url(), None, "there is no default blyg");
    assert!(!c.tutorial_on_launch());
}

#[test]
fn later_values_win_and_empty_resets() {
    let l = load_text("theme = dark\ntheme = light\nlayout = stacked\nlayout =\n");
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    assert_eq!(l.config.theme(), Theme::Light);
    assert_eq!(l.config.layout(), Layout::Side, "empty value resets");
}

#[test]
fn repeatable_keys_make_lists() {
    let l = load_text(
        "ai-enable = anthropic\n\
         ai-enable = OpenAI\n\
         ai-provider-model = anthropic=claude-sonnet-5\n\
         ai-provider-model = cloudflare = @cf/some/model\n",
    );
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    // The first user value replaces the default list (empty: AI is opt-in).
    assert_eq!(l.config.ai_enabled(), vec!["anthropic", "openai"]);
    assert_eq!(
        l.config.ai_provider_models(),
        vec![
            ("anthropic".to_string(), "claude-sonnet-5".to_string()),
            ("cloudflare".to_string(), "@cf/some/model".to_string()),
        ]
    );
    assert!(
        load_text("").config.ai_enabled().is_empty(),
        "no AI provider is on by default"
    );
    assert!(
        load_text("ai-enable =\n").config.ai_enabled().is_empty(),
        "an empty value clears the list"
    );
}

#[test]
fn errors_carry_line_numbers_and_the_rest_still_loads() {
    let l = load_text(
        "theme = dark\n\
         this line has no equals sign\n\
         font-size = huge\n\
         ai-style-prompt = \"unterminated\n\
         layout = diagonal\n\
         font-size = 99\n\
         blyg-url = ftp://example.com\n\
         edited-posts = stay\n",
    );
    let got: Vec<(usize, Severity)> = l.diagnostics.iter().map(|d| (d.line, d.severity)).collect();
    assert_eq!(
        got,
        vec![
            (2, Severity::Error),
            (4, Severity::Error),
            (3, Severity::Error),
            (5, Severity::Error),
            (6, Severity::Warning),
            (7, Severity::Error),
        ],
        "{:#?}",
        l.diagnostics
    );
    assert!(l.diagnostics[0].message.contains("key = value"));
    assert!(l.diagnostics[3].message.contains("side, stacked"));
    assert_eq!(l.config.font_size(), 32.0, "out of range is clamped");
    assert_eq!(l.config.theme(), Theme::Dark);
    assert_eq!(
        l.config.edited_posts(),
        blyg_core::config::EditedPosts::Stay
    );
    assert!(
        l.diagnostics[0]
            .short()
            .ends_with("config:2: expected `key = value`, found `this line has no equals sign`")
    );
}

#[test]
fn unknown_keys_are_warnings_with_a_hint() {
    let l = load_text("fnot-size = 20\nmystery = 1\ntheme = light\n");
    assert_eq!(l.diagnostics.len(), 2);
    assert!(
        l.diagnostics
            .iter()
            .all(|d| d.severity == Severity::Warning)
    );
    assert_eq!(l.diagnostics[0].line, 1);
    assert!(
        l.diagnostics[0]
            .message
            .contains("did you mean `font-size`"),
        "{}",
        l.diagnostics[0].message
    );
    assert!(!l.diagnostics[1].message.contains("did you mean"));
    assert_eq!(l.config.theme(), Theme::Light);
}

#[test]
fn config_file_includes() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("config");
    std::fs::create_dir_all(dir.path().join("more")).unwrap();
    std::fs::write(
        &main,
        "theme = dark\n\
         config-file = more/fonts\n\
         config-file = ?missing-is-fine\n\
         config-file = missing-is-not\n\
         layout = stacked\n",
    )
    .unwrap();
    // Included after the including file, so it overrides it.
    std::fs::write(
        dir.path().join("more/fonts"),
        "font-family-ui = Menlo\ntheme = light\nconfig-file = ../config\n",
    )
    .unwrap();
    let l = parse::load(std::slice::from_ref(&main), &Disk);
    assert_eq!(l.config.font_family_ui(), "Menlo");
    assert_eq!(l.config.theme(), Theme::Light);
    assert_eq!(l.config.layout(), Layout::Stacked);
    assert_eq!(l.files.len(), 2);
    let msgs: Vec<String> = l.diagnostics.iter().map(|d| d.short()).collect();
    assert_eq!(l.diagnostics.len(), 2, "{msgs:#?}");
    // The cycle back to the main file, reported where it's named.
    assert!(l.diagnostics[0].file.ends_with("more/fonts"));
    assert_eq!(l.diagnostics[0].line, 3);
    assert!(l.diagnostics[0].message.contains("already loaded"));
    // A required include that's missing, reported at its line.
    assert_eq!(l.diagnostics[1].line, 4);
    assert!(l.diagnostics[1].message.contains("doesn't exist"));
    assert!(
        l.diagnostics
            .iter()
            .all(|d| !d.message.contains("missing-is-fine"))
    );
}

// ------------------------------------------------------------ write-back

const USER_FILE: &str = "\
# My Blygger settings
theme = dark

# fonts
font-family-writing = Literata
font-family-writing = Charter
my-own-key = kept

ai-enable = anthropic
# between list entries
ai-enable = openai
# trailing comment";

#[test]
fn write_back_preserves_comments_and_ordering() {
    let out = edit::apply(
        USER_FILE,
        &[
            ("theme", Change::Set("light".into())),
            ("font-family-writing", Change::Set("Source Serif 4".into())),
            ("ai-enable", Change::List(vec!["codex".into()])),
            ("layout", Change::Set("stacked".into())),
            ("ai-style-prompt", Change::Set("  two  spaces".into())),
        ],
    );
    assert_eq!(
        out,
        "\
# My Blygger settings
theme = light

# fonts
font-family-writing = Literata
font-family-writing = Source Serif 4
my-own-key = kept

ai-enable = codex
# between list entries
# trailing comment

layout = stacked
ai-style-prompt = \"  two  spaces\"
"
    );
    // What was written reads back as intended.
    let l = load_text(&out);
    assert_eq!(l.config.theme(), Theme::Light);
    assert_eq!(l.config.font_family_writing(), "Source Serif 4");
    assert_eq!(l.config.ai_enabled(), vec!["codex"]);
    assert_eq!(l.config.ai_style_prompt(), Some("  two  spaces"));

    let removed = edit::apply(&out, &[("font-family-writing", Change::Remove)]);
    assert!(!removed.contains("font-family-writing"));
    assert!(removed.contains("# fonts\nmy-own-key = kept"));
    let cleared = edit::apply(&out, &[("ai-enable", Change::List(vec![]))]);
    assert!(cleared.contains("\nai-enable =\n"));
    assert!(load_text(&cleared).config.ai_enabled().is_empty());
}

#[test]
fn store_writes_each_key_where_it_was_set() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg/config");
    let support = dir.path().join("support/config");
    std::fs::create_dir_all(xdg.parent().unwrap()).unwrap();
    std::fs::write(&xdg, "# xdg\ntheme = dark\nconfig-file = ?fonts\n").unwrap();
    std::fs::write(xdg.with_file_name("fonts"), "font-size = 17\n").unwrap();
    let files = ConfigFiles {
        load: vec![xdg.clone(), support.clone()],
        primary: xdg.clone(),
    };
    let mut store = ConfigStore::open(files);
    assert_eq!(store.config().font_size(), 17.0);
    store
        .set(&[
            ("theme", Change::Set("light".into())),
            ("font-size", Change::Set("20".into())),
            ("layout", Change::Set("stacked".into())),
        ])
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&xdg).unwrap(),
        "# xdg\ntheme = light\nconfig-file = ?fonts\nlayout = stacked\n"
    );
    assert_eq!(
        std::fs::read_to_string(xdg.with_file_name("fonts")).unwrap(),
        "font-size = 20\n"
    );
    assert!(!support.exists());
    assert_eq!(store.config().theme(), Theme::Light);
    assert_eq!(store.config().font_size(), 20.0);

    // A missing primary file is created with a commented header.
    let fresh = dir.path().join("new/config");
    let mut s = ConfigStore::open(ConfigFiles::single(fresh.clone()));
    s.set(&[("blyg-url", Change::Set("https://blyg.example.com".into()))])
        .unwrap();
    let text = std::fs::read_to_string(&fresh).unwrap();
    assert!(text.starts_with("# Blygger configuration."), "{text}");
    assert!(
        text.ends_with("\nblyg-url = https://blyg.example.com\n"),
        "{text}"
    );
    assert_eq!(s.config().blyg_url(), Some("https://blyg.example.com"));
}

#[test]
fn in_memory_store_uses_the_same_write_back() {
    let mut s = ConfigStore::in_memory("# mine\ntheme = dark\n");
    s.set(&[("theme", Change::Set("light".into()))]).unwrap();
    assert_eq!(s.text(), Some("# mine\ntheme = light\n"));
    assert_eq!(s.config().theme(), Theme::Light);
}

// ------------------------------------------------------------ locations

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |k| {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(v))
    }
}

#[test]
fn xdg_then_app_support_in_that_order() {
    let none = |_: &Path| false;
    let f = ConfigFiles::discover_with(&env(&[("HOME", "/home/u")]), &none);
    assert_eq!(
        f.load,
        vec![
            PathBuf::from("/home/u/.config/blygger/config"),
            PathBuf::from("/home/u/Library/Application Support/org.blygger.desktop/config"),
        ]
    );
    assert_eq!(
        f.primary, f.load[0],
        "nothing exists: write to the XDG file"
    );

    let f = ConfigFiles::discover_with(
        &env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/xdg")]),
        &none,
    );
    assert_eq!(f.load[0], PathBuf::from("/xdg/blygger/config"));
    // A relative XDG_CONFIG_HOME is ignored, per the spec.
    let f = ConfigFiles::discover_with(
        &env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "rel")]),
        &none,
    );
    assert_eq!(f.load[0], PathBuf::from("/home/u/.config/blygger/config"));

    // The later existing file is the primary (it wins on conflicts).
    let both = |_: &Path| true;
    let f = ConfigFiles::discover_with(&env(&[("HOME", "/home/u")]), &both);
    assert_eq!(f.primary, f.load[1]);

    let f = ConfigFiles::discover_with(
        &env(&[("HOME", "/home/u"), ("BLYGGER_CONFIG", "/tmp/scratch")]),
        &both,
    );
    assert_eq!(f, ConfigFiles::single(PathBuf::from("/tmp/scratch")));

    assert_eq!(
        data_dir_with(&env(&[("HOME", "/home/u")])),
        PathBuf::from("/home/u/Library/Application Support/org.blygger.desktop")
    );
    assert_eq!(
        data_dir_with(&env(&[("HOME", "/home/u"), ("BLYGGER_DATA_DIR", "/d")])),
        PathBuf::from("/d")
    );
}

#[test]
fn later_location_overrides_earlier() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::write(&a, "theme = dark\nlayout = stacked\n").unwrap();
    std::fs::write(&b, "theme = light\n").unwrap();
    let l = parse::load(&[a, b], &Disk);
    assert_eq!(l.config.theme(), Theme::Light);
    assert_eq!(l.config.layout(), Layout::Stacked);
}

// ------------------------------------------------------------ migration

#[test]
fn data_dir_is_moved_not_copied() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("Blygger");
    let new = dir.path().join("org.blygger.desktop");
    std::fs::create_dir_all(old.join("media")).unwrap();
    std::fs::write(old.join("blygger.db"), "db").unwrap();
    std::fs::write(old.join("media/a.png"), "png").unwrap();
    let moved = migrate_data_dir(&old, &new).unwrap();
    assert_eq!(moved.len(), 2);
    assert_eq!(
        std::fs::read_to_string(new.join("blygger.db")).unwrap(),
        "db"
    );
    assert!(new.join("media/a.png").exists());
    assert!(!old.exists(), "the emptied old dir is removed");
    assert!(migrate_data_dir(&old, &new).unwrap().is_empty());
}

#[test]
fn legacy_toml_files_become_one_config() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let target = dir.path().join("xdg/blygger/config");
    std::fs::write(
        data.join("ui.toml"),
        "# my fonts\nwriting_font = \"Source Serif 4\"\nui_font = \"Inter\"\nfont_size = 21.0\ntheme = \"dark\"\nhotkey = \"ctrl+alt+B\"\n",
    )
    .unwrap();
    std::fs::write(
        data.join("config.toml"),
        "base_url = \"https://blyg.example.com\"\n[ui]\nsomething_else = 1\n",
    )
    .unwrap();
    std::fs::write(
        data.join("ai.toml"),
        "default_provider = \"anthropic-api\"\ncloudflare_account_id = \"acc123\"\nchatgpt_notice_shown = true\n\n[providers.anthropic-api]\nenabled = true\nmodel = \"claude-sonnet-5\"\n\n[providers.local-codex]\nenabled = false\n\n[providers.openai-api]\nmodel = \"gpt-x\"\n",
    )
    .unwrap();

    let r = migrate_legacy_files(data, &target).unwrap().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.migrated.len(), 3);
    for f in ["ui.toml", "ai.toml", "config.toml"] {
        assert!(!data.join(f).exists());
        assert!(data.join(format!("{f}.migrated")).exists());
    }
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.starts_with("# Blygger configuration."));
    assert!(
        text.contains("# Migrated from ui.toml\n# my fonts\n"),
        "{text}"
    );
    assert!(
        text.contains("# not migrated: [ui] something_else = 1"),
        "{text}"
    );
    let l = parse::load(std::slice::from_ref(&target), &Disk);
    assert!(l.diagnostics.is_empty(), "{:?}\n{text}", l.diagnostics);
    let c = &l.config;
    assert_eq!(c.blyg_url(), Some("https://blyg.example.com"));
    assert_eq!(c.font_family_writing(), "Source Serif 4");
    assert_eq!(c.font_size(), 21.0);
    assert_eq!(c.theme(), Theme::Dark);
    assert_eq!(c.capture_hotkey(), "ctrl+alt+b");
    assert_eq!(c.ai_provider(), Some("anthropic"));
    assert_eq!(c.ai_model(), Some("claude-sonnet-5"));
    assert_eq!(
        c.ai_provider_models(),
        vec![("openai".to_string(), "gpt-x".to_string())]
    );
    assert_eq!(c.ai_enabled(), vec!["claude-code", "anthropic"]);
    assert_eq!(c.cloudflare_account_id(), Some("acc123"));
    assert!(
        AppState::load(data).chatgpt_notice_shown,
        "state, not config"
    );
    assert!(!text.contains("chatgpt"), "{text}");

    // Once only.
    assert!(migrate_legacy_files(data, &target).unwrap().is_none());
}

#[test]
fn migration_keeps_existing_settings_and_skips_broken_files() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let target = data.join("config");
    std::fs::write(&target, "theme = light\n").unwrap();
    std::fs::write(data.join("ui.toml"), "theme = \"dark\"\nfont_size = 15\n").unwrap();
    std::fs::write(data.join("ai.toml"), "default_provider = [").unwrap();
    let r = migrate_legacy_files(data, &target).unwrap().unwrap();
    assert_eq!(r.migrated.len(), 1);
    assert_eq!(r.problems.len(), 1);
    assert!(data.join("ai.toml").exists(), "a broken file is left alone");
    let l = parse::load(std::slice::from_ref(&target), &Disk);
    assert_eq!(l.config.theme(), Theme::Light, "the new file wins");
    assert_eq!(l.config.font_size(), 15.0);
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("# already set above: theme = dark"), "{text}");
}

// ------------------------------------------------------------ show-config

#[test]
fn show_config_default_docs_lists_every_key() {
    let out = show_config(
        &Default::default(),
        ShowOptions::from_args(["--default", "--docs"]).unwrap(),
    );
    for k in KEYS {
        assert!(
            out.lines().any(|l| l.starts_with(&format!("{} =", k.name))),
            "{} missing:\n{out}",
            k.name
        );
    }
    assert!(out.starts_with("# The blyg this app writes to"), "{out}");
    assert!(out.contains("\nblyg-url =\n"), "no default blyg");
    assert!(out.contains("\ntheme = system\n"));
    assert!(out.contains("\ncapture-hotkey = ctrl+alt+b\n"));
    assert!(out.contains("\nai-enable =\n"), "AI is opt-in");
    assert!(out.contains("\nai-disclose = always\n"));
    // Every doc line is a comment and short enough to read in a terminal.
    for l in out.lines().filter(|l| l.starts_with('#')) {
        assert!(l.chars().count() <= 80, "too long: {l}");
    }
    // What it prints is itself a valid config file.
    let l = load_text(&out);
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);

    let plain = show_config(
        &Default::default(),
        ShowOptions::from_args(["--default"]).unwrap(),
    );
    assert!(!plain.contains('#'));
    assert_eq!(
        plain.lines().count(),
        out.lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count()
    );
    assert!(ShowOptions::from_args(["--bogus"]).is_err());
}

#[test]
fn show_config_prints_the_effective_changes() {
    let l = load_text("# c\ntheme = dark\nai-style-prompt = \" padded \"\nbogus = 1\n");
    let out = show_config(&l.config, ShowOptions::default());
    assert_eq!(out, "theme = dark\nai-style-prompt = \" padded \"\n");
    let all = show_config(
        &l.config,
        ShowOptions::from_args(["--changes-only=false"]).unwrap(),
    );
    assert!(all.contains("theme = dark\n") && all.contains("layout = side\n"));
}

// ------------------------------------------------------------ scratch notes

#[test]
fn capture_default_and_new_note_parse() {
    use blyg_core::config::{CaptureDefault, NewNote};
    let l = load_text("");
    assert_eq!(l.config.capture_default(), CaptureDefault::Scratch);
    assert_eq!(l.config.new_note(), NewNote::Draft);

    let l = load_text("capture-default = Draft\nnew-note = scratch\n");
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    assert_eq!(l.config.capture_default(), CaptureDefault::Draft);
    assert_eq!(l.config.new_note(), NewNote::Scratch);

    let l = load_text("capture-default = publish\nnew-note = later\n");
    assert_eq!(l.diagnostics.len(), 2, "{:?}", l.diagnostics);
    assert!(l.diagnostics.iter().all(|d| d.severity == Severity::Error));
    assert!(l.diagnostics[0].message.contains("scratch, draft"));
    assert_eq!(l.config.capture_default(), CaptureDefault::Scratch);
    assert_eq!(l.config.new_note(), NewNote::Draft);

    let out = show_config(
        &Default::default(),
        ShowOptions::from_args(["--default", "--docs"]).unwrap(),
    );
    assert!(out.contains("\ncapture-default = scratch\n"), "{out}");
    assert!(out.contains("\nnew-note = draft\n"), "{out}");
}

// ------------------------------------------------------------ buttons

#[test]
fn show_buttons_defaults_on_and_is_documented() {
    assert!(load_text("").config.show_buttons(), "default true");
    let l = load_text("show-buttons = FALSE\n");
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    assert!(!l.config.show_buttons());
    let l = load_text("show-buttons = maybe\n");
    assert_eq!(l.diagnostics.len(), 1, "{:?}", l.diagnostics);
    assert!(l.config.show_buttons(), "a bad value keeps the default");
    let out = show_config(
        &Default::default(),
        ShowOptions::from_args(["--default", "--docs"]).unwrap(),
    );
    assert!(out.contains("\nshow-buttons = true\n"), "{out}");
    assert!(out.contains("tooltip shows its shortcut"), "{out}");
}
