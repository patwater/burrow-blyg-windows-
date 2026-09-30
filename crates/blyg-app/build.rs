//! Windows only: embed the icon Explorer and the taskbar show into
//! blygger.exe. The manifest (per-monitor DPI awareness, common controls v6)
//! comes from GPUI's own `windows-manifest` feature, and a second one would
//! be a duplicate resource. Other targets need nothing here.

use std::path::PathBuf;

fn main() {
    let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../packaging/Blygger.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // An absolute path, so neither rc.exe nor windres has to guess what a
    // relative one is relative to.
    let icon = icon.canonicalize().unwrap_or(icon);
    let rc = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("blygger.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
