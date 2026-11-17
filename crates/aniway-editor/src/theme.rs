//! Themes (graphite, midnight and paper bases, two accents each) that skin both the native widgets, through CSS
//! variables, and the map drawing; plus the bundled fonts and icons.

use adw::prelude::*;
use gtk::{gdk, pango};
use pango::prelude::FontMapExt;
use std::cell::{Cell, RefCell};

const FONTS: [(&str, &[u8]); 3] = [
    ("SpaceGrotesk.ttf", include_bytes!("../resources/fonts/SpaceGrotesk.ttf")),
    ("DMSans.ttf", include_bytes!("../resources/fonts/DMSans.ttf")),
    ("JetBrainsMono.ttf", include_bytes!("../resources/fonts/JetBrainsMono.ttf")),
];

const ICONS: [(&str, &str); 7] = [
    ("apps/aniway-light.svg", include_str!("../resources/icons/aniway-light.svg")),
    ("apps/aniway-dark.svg", include_str!("../resources/icons/aniway-dark.svg")),
    ("actions/aniway-placement-symbolic.svg", include_str!("../resources/icons/aniway-placement-symbolic.svg")),
    ("actions/aniway-size-symbolic.svg", include_str!("../resources/icons/aniway-size-symbolic.svg")),
    ("actions/aniway-format-symbolic.svg", include_str!("../resources/icons/aniway-format-symbolic.svg")),
    ("actions/aniway-select-symbolic.svg", include_str!("../resources/icons/aniway-select-symbolic.svg")),
    ("actions/aniway-scenes-symbolic.svg", include_str!("../resources/icons/aniway-scenes-symbolic.svg")),
];

/// The logo in the colour that stands out on the current theme: light strokes on dark themes, dark on light ones.
pub fn logo() -> &'static str {
    if adw::StyleManager::default().is_dark() {
        "aniway-light"
    } else {
        "aniway-dark"
    }
}

/// Display font for headings, numbers and labels on the map.
pub const DISPLAY: &str = "Space Grotesk";
/// Font of measurements.
pub const MONO: &str = "JetBrains Mono";

/// Sticker colours of the overlays, in scene order; shared by the map chips and the list badges.
pub const VINYL: [u32; 6] = [0xff5a5f, 0xffd23f, 0x3bceac, 0x5f6cff, 0xc77dff, 0xff9f45];
/// Near-black ink that reads at >= 4:1 on every vinyl colour, including blue.
pub const VINYL_INK: u32 = 0x1d1f23;

pub struct Theme {
    pub id: &'static str,
    pub dark: bool,
    pub window: u32,
    pub centre: u32,
    inspector: u32,
    card: u32,
    border: u32,
    pub accent: u32,
    /// Accent for text and outlines on the window background.
    accent_text: u32,
    /// Text on the accent colour.
    pub accent_fg: u32,
    lid: u32,
    pub screen: u32,
    screen_top: u32,
    mini: u32,
    mute: u32,
    /// Accent tint that stays legible on the dark screen: selection ring, guides and dimensions.
    edge: u32,
}

const fn graphite(id: &'static str, accent: u32, accent_text: u32, accent_fg: u32) -> Theme {
    Theme {
        id,
        dark: true,
        window: 0x232529,
        centre: 0x2e3136,
        inspector: 0x272a2e,
        card: 0x33363c,
        border: 0x41454c,
        accent,
        accent_text,
        accent_fg,
        lid: 0x50555c,
        screen: 0x1f2226,
        screen_top: 0x3d434c,
        mini: 0x1c1e22,
        mute: 0xa9aeb6,
        edge: accent_text,
    }
}

const fn midnight(id: &'static str, accent: u32, accent_text: u32, accent_fg: u32) -> Theme {
    Theme {
        id,
        dark: true,
        window: 0x0e0f11,
        centre: 0x15171a,
        inspector: 0x111215,
        card: 0x1b1d21,
        border: 0x2a2d33,
        accent,
        accent_text,
        accent_fg,
        lid: 0x2b2e34,
        screen: 0x08090b,
        screen_top: 0x1c2027,
        mini: 0x22252a,
        mute: 0x8f959e,
        edge: accent_text,
    }
}

const fn paper(id: &'static str, accent: u32, accent_text: u32, edge: u32) -> Theme {
    Theme {
        id,
        dark: false,
        window: 0xf6f6f4,
        centre: 0xe9e9e6,
        inspector: 0xfbfbfa,
        card: 0xffffff,
        border: 0xdadad6,
        accent,
        accent_text,
        accent_fg: 0xffffff,
        lid: 0xcfd2d7,
        screen: 0x2b2f35,
        screen_top: 0x454b54,
        mini: 0xcfd2d7,
        mute: 0x5b616a,
        edge,
    }
}

/// Themes used when following the system style.
pub const SYSTEM_DARK: usize = 0;
pub const SYSTEM_LIGHT: usize = 5;

pub const THEMES: [Theme; 7] = [
    graphite("graphite-coral", 0xff5a5f, 0xff8a8d, 0xffffff),
    graphite("graphite-amber", 0xffc531, 0xffd23f, VINYL_INK),
    midnight("midnight-violet", 0x8b6cff, 0xb6a2ff, 0xffffff),
    midnight("midnight-mint", 0x2fd4a7, 0x5fe3bf, 0x05261d),
    midnight("midnight-white", 0xf2f2f2, 0xffffff, 0x0e0f11),
    paper("paper-coral", 0xe5484d, 0xbf2c33, 0xff8a8d),
    paper("paper-blue", 0x3e63dd, 0x2d4fbf, 0x93aaff),
];

/// Colours of the map drawing.
pub struct Palette {
    pub lid: gdk::RGBA,
    pub screen: gdk::RGBA,
    pub screen_top: gdk::RGBA,
    pub mini: gdk::RGBA,
    pub mute: gdk::RGBA,
    /// Accent for text and focus outlines on the lid and the canvas.
    pub label: gdk::RGBA,
    /// Accent for strokes and text on the dark screen.
    pub edge: gdk::RGBA,
    pub keyline: gdk::RGBA,
    pub dark: bool,
}

pub fn rgb(hex: u32) -> gdk::RGBA {
    gdk::RGBA::new((hex >> 16 & 255) as f32 / 255.0, (hex >> 8 & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0, 1.0)
}

fn css(hex: u32) -> String {
    format!("#{hex:06x}")
}

thread_local! {
    static CHOSEN: RefCell<String> = RefCell::default();
    static CURRENT: Cell<usize> = const { Cell::new(0) };
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = RefCell::default();
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = RefCell::default();
}

pub fn current() -> &'static Theme {
    &THEMES[CURRENT.get()]
}

pub fn palette() -> Palette {
    let t = current();
    Palette {
        lid: rgb(t.lid),
        screen: rgb(t.screen),
        screen_top: rgb(t.screen_top),
        mini: rgb(t.mini),
        mute: rgb(t.mute),
        label: rgb(t.accent_text),
        edge: rgb(t.edge),
        keyline: rgb(0xffffff),
        dark: t.dark,
    }
}

/// Runs `f` after every theme change (custom drawings redraw there).
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with_borrow_mut(|l| l.push(Box::new(f)));
}

const COMMON: &str = "
.heading, .title-1, .title-2, .title-3, .title-4, headerbar .title, .section-title, .vinyl, .keycap, .display,
  .scene-name, dialog .title { font-family: 'Space Grotesk', sans-serif; }
.cols, .mono, spinbutton > text, .readout { font-family: 'JetBrains Mono', monospace; font-size: 11px; }
.centre { background: var(--centre-bg); }
.inspector { background: var(--inspector-bg); }
.overlay-list { background: transparent; padding: 6px 0; }
.overlay-list > row { margin: 3px 10px; padding: 0; border-radius: 14px; }
.overlay-list > row:selected { background: alpha(var(--accent-bg-color), 0.16); }
.overlay-list > row:selected:hover { background: alpha(var(--accent-bg-color), 0.22); }
.row-title { font-weight: 600; font-size: 14px; }
.meta { font-size: 12px; color: var(--mute); }
.meta image { opacity: 0.8; }
.vinyl { border-radius: 7px; padding: 0 7px; font-weight: 700; font-size: 11px; color: #1d1f23; letter-spacing: 0.04em; }
.vinyl-0 { background: #ff5a5f; } .vinyl-1 { background: #ffd23f; } .vinyl-2 { background: #3bceac; }
.vinyl-3 { background: #5f6cff; } .vinyl-4 { background: #c77dff; } .vinyl-5 { background: #ff9f45; }
.row-delete { opacity: 0.4; }
row:hover .row-delete, row:selected .row-delete, .row-delete:focus-visible { opacity: 1; }
.state { font-size: 12px; font-weight: 500; color: var(--warning-color); }
.state.error { color: var(--error-color); }
.thumb { border-radius: 12px; background: alpha(currentColor, 0.08); }
.list-footer { color: var(--mute); }
button.dropzone { border: 2px dashed alpha(var(--accent-bg-color), 0.55); border-radius: 18px; padding: 10px 12px 12px;
  background: alpha(var(--accent-bg-color), 0.05); box-shadow: none; transition: all 150ms ease-out; }
button.dropzone:hover { background: alpha(var(--accent-bg-color), 0.1); }
button.dropzone.active { border-style: solid; border-color: var(--accent-bg-color); background: alpha(var(--accent-bg-color), 0.18); }
.dropzone .title { font-weight: 700; font-size: 14px; }
.dropzone .caption { color: var(--mute); }
.hintbar { padding: 6px 16px 14px; color: var(--mute); font-size: 12px; }
.keycap { font-size: 11px; font-weight: 600; padding: 0 6px; min-width: 12px; border-radius: 6px;
  border: 1px solid alpha(currentColor, 0.3); border-bottom-width: 2px; background: alpha(currentColor, 0.06); }
.preview { border-radius: 16px; background: alpha(currentColor, 0.08); }
.preview-toggle { margin: 8px; min-width: 28px; min-height: 28px; padding: 0; background: rgba(0, 0, 0, 0.55); color: #fff; }
.section-title { font-weight: 700; font-size: 13px; }
.section-icon { color: var(--accent-color); }
.inspector-name { font-family: 'Space Grotesk', sans-serif; font-weight: 700; font-size: 19px; }
.anchor:checked { background: var(--accent-bg-color); color: var(--accent-fg-color); }
.scene-row { padding: 2px 2px 2px 6px; }
.scene-row > checkbutton > label { margin-left: 8px; }
popover.scenes > contents { padding: 8px; }
.theme-card { padding: 6px; border-radius: 16px; }
.theme-card:checked { box-shadow: 0 0 0 2px var(--accent-bg-color); background: alpha(var(--accent-bg-color), 0.08); }
.theme-card label { font-size: 12px; }
preferencespage > scrolledwindow > viewport > clamp > box { margin: 18px 24px 30px; }
dialog.shortcuts toolbarview > .top-bar headerbar > windowhandle > box { padding-top: 12px; padding-bottom: 8px; }
";

/// CSS of a theme: every libadwaita colour, both as variables and as the older named colours, so a system GTK theme
/// (pywal, matugen, Noctalia… in `~/.config/gtk-4.0/gtk.css`) cannot leak into the editor; plus the pane colours used
/// by `COMMON`.
fn variables(t: &Theme) -> String {
    let (w, card, border, accent) = (css(t.window), css(t.card), css(t.border), css(t.accent));
    let fg = if t.dark { "#ffffff" } else { "rgba(0, 0, 6, 0.8)" };
    let (destructive, success, warning) =
        if t.dark { ("#ff7b63", "#78e9ab", "#ffc252") } else { ("#c30000", "#007c3d", "#9c6e03") };
    let shade = if t.dark { "rgba(0, 0, 6, 0.25)" } else { "rgba(0, 0, 6, 0.07)" };
    let colours = [
        ("accent-bg", accent.clone()),
        ("accent", css(t.accent_text)),
        ("accent-fg", css(t.accent_fg)),
        ("window-bg", w.clone()),
        ("window-fg", fg.into()),
        ("view-bg", w.clone()),
        ("view-fg", fg.into()),
        ("headerbar-bg", w.clone()),
        ("headerbar-fg", fg.into()),
        ("headerbar-backdrop", w.clone()),
        ("headerbar-border", border.clone()),
        ("headerbar-shade", shade.into()),
        ("popover-bg", card.clone()),
        ("popover-fg", fg.into()),
        ("card-bg", card.clone()),
        ("card-fg", fg.into()),
        ("dialog-bg", w.clone()),
        ("dialog-fg", fg.into()),
        ("overview-bg", card.clone()),
        ("overview-fg", fg.into()),
        ("sidebar-bg", w.clone()),
        ("sidebar-fg", fg.into()),
        ("sidebar-backdrop", w.clone()),
        ("sidebar-border", border.clone()),
        ("sidebar-shade", shade.into()),
        ("secondary-sidebar-bg", w.clone()),
        ("secondary-sidebar-fg", fg.into()),
        ("secondary-sidebar-backdrop", w.clone()),
        ("secondary-sidebar-border", border.clone()),
        ("thumbnail-bg", card.clone()),
        ("thumbnail-fg", fg.into()),
        ("destructive-bg", "#e01b24".into()),
        ("destructive-fg", "#ffffff".into()),
        ("destructive", destructive.into()),
        ("error-bg", "#e01b24".into()),
        ("error-fg", "#ffffff".into()),
        ("error", destructive.into()),
        ("success-bg", "#2ec27e".into()),
        ("success-fg", "#ffffff".into()),
        ("success", success.into()),
        ("warning-bg", "#e5a50a".into()),
        ("warning-fg", "rgba(0, 0, 0, 0.8)".into()),
        ("warning", warning.into()),
        ("shade", shade.into()),
    ];
    let mut out = String::from(":root {");
    for (name, value) in &colours {
        out += &format!(" --{name}-color: {value};");
    }
    out += &format!(
        " --centre-bg: {}; --inspector-bg: {}; --mute: {}; }}\n",
        css(t.centre),
        css(t.inspector),
        css(t.mute)
    );
    for (name, value) in &colours {
        out += &format!("@define-color {}_color {value};\n", name.replace('-', "_"));
    }
    out
}

/// Applies the theme `id` (unknown or empty: follow the system style with the coral themes).
pub fn apply(id: &str) {
    CHOSEN.set(id.to_string());
    let manager = adw::StyleManager::default();
    let fixed = THEMES.iter().position(|t| t.id == id);
    manager.set_color_scheme(match fixed.map(|i| THEMES[i].dark) {
        Some(true) => adw::ColorScheme::ForceDark,
        Some(false) => adw::ColorScheme::ForceLight,
        None => adw::ColorScheme::Default,
    });
    let index = fixed.unwrap_or(if manager.is_dark() { SYSTEM_DARK } else { SYSTEM_LIGHT });
    CURRENT.set(index);
    PROVIDER.with_borrow(|p| p.as_ref().map(|p| p.load_from_string(&variables(&THEMES[index]))));
    LISTENERS.with_borrow(|l| l.iter().for_each(|f| f()));
}

/// Installs the CSS, the bundled fonts and icons, and the theme chosen in the settings.
pub fn install(theme: &str) {
    let Some(display) = gdk::Display::default() else { return };
    let common = gtk::CssProvider::new();
    common.load_from_string(COMMON);
    // Above USER priority: a system GTK theme in ~/.config/gtk-4.0/gtk.css would otherwise repaint the chosen theme.
    gtk::style_context_add_provider_for_display(&display, &common, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
    let variant = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &variant, gtk::STYLE_PROVIDER_PRIORITY_USER + 2);
    PROVIDER.set(Some(variant));
    install_assets(&display);
    apply(theme);
    adw::StyleManager::default().connect_dark_notify(|_| {
        if !THEMES.iter().any(|t| CHOSEN.with_borrow(|c| *c == t.id)) {
            apply("");
        }
    });
}

/// Copies the fonts and icons to the cache (Pango and the icon theme only load files) and registers them; any
/// failure only means system fonts or missing icons.
fn install_assets(display: &gdk::Display) {
    let root = aniway_proto::paths::cache_dir();
    let write = |path: std::path::PathBuf, data: &[u8]| -> std::io::Result<std::path::PathBuf> {
        std::fs::create_dir_all(path.parent().unwrap_or(&root))?;
        if std::fs::metadata(&path).map_or(true, |m| m.len() != data.len() as u64) {
            std::fs::write(&path, data)?;
        }
        Ok(path)
    };
    let map = gtk::Label::new(None).pango_context().font_map();
    for (name, data) in FONTS {
        let added = write(root.join("fonts").join(name), data).and_then(|f| match &map {
            Some(map) => map.add_font_file(&f).map_err(|e| std::io::Error::other(e.to_string())),
            None => Err(std::io::Error::other("no font map")),
        });
        if let Err(e) = added {
            log::warn!("cannot register {name}: {e}");
        }
    }
    for (name, data) in ICONS {
        if let Err(e) = write(root.join("icons/hicolor/scalable").join(name), data.as_bytes()) {
            log::warn!("cannot install icon {name}: {e}");
        }
    }
    gtk::IconTheme::for_display(display).add_search_path(root.join("icons"));
    // Keep the size the user chose for the interface font; only the family changes.
    if let Some(settings) = gtk::Settings::default() {
        let size = settings.gtk_font_name().and_then(|f| f.rsplit(' ').next()?.parse::<f64>().ok()).unwrap_or(11.0);
        settings.set_gtk_font_name(Some(&format!("DM Sans {size}")));
    }
}
