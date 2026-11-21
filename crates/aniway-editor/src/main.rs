mod client;
mod dropzone;
mod inspector;
mod library;
mod list;
mod map;
mod media;
mod prefs;
mod scenes;
mod settings;
mod theme;
mod window;

use adw::prelude::*;
use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};
pub use gettextrs::{gettext as tr, ngettext};

const DOMAIN: &str = "aniway";

/// Catalogues are read from `<prefix>/share/locale`, or from `target/locale` next to the binary's directory.
fn init_i18n() {
    setlocale(LocaleCategory::LcAll, "");
    let exe = std::env::current_exe().unwrap_or_default();
    let dirs = ["../share/locale", "../locale"].map(|d| exe.with_file_name(d));
    if let Some(dir) = dirs.iter().find(|d| d.is_dir()) {
        let _ = bindtextdomain(DOMAIN, dir);
    }
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

fn main() -> gtk::glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // The Cairo renderer needs far less memory than Vulkan or GL and draws this UI identically.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    for note in aniway_proto::legacy::migrate(prefs::sibling_daemon().as_deref()) {
        log::info!("{note}");
    }
    let settings = settings::load();
    // gettext reads LANGUAGE before the locale, so a chosen language wins over the system one.
    if !settings.language.is_empty() {
        std::env::set_var("LANGUAGE", &settings.language);
    }
    init_i18n();
    let app = adw::Application::builder().application_id("io.github.axl_std.AniWay").build();
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
        } else {
            theme::install(&settings.theme);
            window::build(app).present();
        }
    });
    app.run()
}
