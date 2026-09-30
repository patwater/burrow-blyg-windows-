//! Bundled OFL fonts (see `assets/fonts/*/` for licenses). Embedded in the
//! binary, but registered with the text system lazily: only the families in
//! use are handed to CoreText at launch, which keeps cold start fast.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bundle {
    Literata,
    Inter,
    SourceSerif,
    Quattro,
    EtBook,
}

macro_rules! font {
    ($p:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/", $p))
    };
}

pub(crate) static LITERATA: &[&[u8]] = &[
    font!("literata/Literata-Regular.ttf"),
    font!("literata/Literata-Italic.ttf"),
    font!("literata/Literata-SemiBold.ttf"),
    font!("literata/Literata-Bold.ttf"),
    font!("literata/Literata-BoldItalic.ttf"),
];
static INTER: &[&[u8]] = &[
    font!("inter/Inter-Regular.ttf"),
    font!("inter/Inter-Italic.ttf"),
    font!("inter/Inter-Medium.ttf"),
    font!("inter/Inter-SemiBold.ttf"),
    font!("inter/Inter-Bold.ttf"),
];
static SOURCE_SERIF: &[&[u8]] = &[
    font!("source-serif-4/SourceSerif4-Regular.ttf"),
    font!("source-serif-4/SourceSerif4-It.ttf"),
    font!("source-serif-4/SourceSerif4-Semibold.ttf"),
    font!("source-serif-4/SourceSerif4-Bold.ttf"),
    font!("source-serif-4/SourceSerif4-BoldIt.ttf"),
];
static QUATTRO: &[&[u8]] = &[
    font!("ia-writer-quattro/iAWriterQuattroS-Regular.ttf"),
    font!("ia-writer-quattro/iAWriterQuattroS-Italic.ttf"),
    font!("ia-writer-quattro/iAWriterQuattroS-Bold.ttf"),
    font!("ia-writer-quattro/iAWriterQuattroS-BoldItalic.ttf"),
];
static ET_BOOK: &[&[u8]] = &[
    font!("et-book/et-book-roman-line-figures.ttf"),
    font!("et-book/et-book-display-italic-old-style-figures.ttf"),
    font!("et-book/et-book-bold-line-figures.ttf"),
];

impl Bundle {
    fn files(self) -> &'static [&'static [u8]] {
        match self {
            Bundle::Literata => LITERATA,
            Bundle::Inter => INTER,
            Bundle::SourceSerif => SOURCE_SERIF,
            Bundle::Quattro => QUATTRO,
            Bundle::EtBook => ET_BOOK,
        }
    }
}

static LOADED: Mutex<Option<HashSet<Bundle>>> = Mutex::new(None);

/// Register a bundle with GPUI's text system once.
pub fn ensure(bundle: Option<Bundle>, cx: &gpui_kit::App) {
    let Some(bundle) = bundle else { return };
    let mut guard = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    if !set.insert(bundle) {
        return;
    }
    let fonts: Vec<Cow<'static, [u8]>> = bundle.files().iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("blygger: could not load bundled font {bundle:?}: {e}");
    }
}
