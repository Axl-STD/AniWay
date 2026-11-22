//! Window shell: header bar, the three panes, toasts, and the daemon-down and connection-lost states.

use crate::client::{self, Ctl};
use crate::inspector::Inspector;
use crate::list::List;
use crate::map::MonitorMap;
use crate::prefs::{sibling_daemon, Prefs};
use crate::scenes::SceneMenu;
use crate::tr;
use adw::prelude::*;
use aniway_proto::autostart;
use aniway_proto::ipc::Request;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

const SNOOZE_SECS: u32 = 15 * 60;
pub const REPO: &str = "https://github.com/Axl-STD/AniWay";
const REPO_SHORT: &str = "github.com/Axl-STD/AniWay";

/// Starts the daemon: through the systemd unit when autostart is enabled, else detached from this process.
fn start_daemon() {
    let mut cmd = if autostart::status().enabled {
        let mut c = Command::new("systemctl");
        c.args(["--user", "start", "aniway.service"]);
        c
    } else {
        let mut c = Command::new("setsid");
        c.arg("-f").arg(sibling_daemon().unwrap_or_else(|| "aniwayd".into()));
        c
    };
    if let Err(e) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        log::error!("cannot start the daemon: {e}");
    }
}

fn add_files(ctl: &Rc<Ctl>, window: &adw::ApplicationWindow) {
    let dialog = gtk::FileDialog::builder().title(tr("Add media")).build();
    let ctl = ctl.clone();
    dialog.open_multiple(Some(window), gio::Cancellable::NONE, move |files| {
        let Ok(files) = files else { return };
        ctl.add_paths(
            files
                .iter::<gio::File>()
                .flatten()
                .filter_map(|f| Some(f.path()?.to_string_lossy().into_owned()))
                .collect(),
        );
    });
}

fn shortcuts() -> adw::ShortcutsDialog {
    let section = adw::ShortcutsSection::new(None);
    for (title, keys) in [
        (tr("Nudge the selected overlay by 1 px"), "Left Right Up Down"),
        (tr("Nudge by 10 px"), "<Shift>Left <Shift>Right <Shift>Up <Shift>Down"),
        (tr("Disable snapping while dragging"), "Alt_L"),
        (tr("Select the next overlay"), "Tab"),
        (tr("Select the previous overlay"), "<Shift>Tab"),
        (tr("Show monitor 1 to 9"), "<Control>1 <Control>2 <Control>9"),
        (tr("Remove the selected overlay (asks first)"), "Delete"),
        (tr("Add media"), "<Control>o"),
        (tr("New scene"), "<Control>n"),
        (tr("Preferences"), "<Control>comma"),
        (tr("Keyboard shortcuts"), "<Control>question"),
    ] {
        section.add(adw::ShortcutsItem::new(&title, keys));
    }
    let dialog = adw::ShortcutsDialog::new();
    dialog.add(section);
    dialog
}

fn about() -> adw::AboutDialog {
    let dialog = adw::AboutDialog::builder()
        .application_name("AniWay")
        .application_icon(crate::theme::logo())
        .developer_name(REPO_SHORT)
        .version(env!("CARGO_PKG_VERSION"))
        .website(REPO)
        .issue_url(format!("{REPO}/issues"))
        .copyright("© 2026 Axl-STD")
        .license_type(gtk::License::Gpl30)
        .comments(tr("Videos, GIFs and images as click-through overlays on your Hyprland monitors. Place them on a map of your screens, group them in scenes with their own shortcuts, and choose for each one whether it stays above fullscreen apps or under your windows.\n\nA small background service draws them with libmpv; this editor only controls it, so the overlays keep running with the editor closed."))
        .build();
    dialog.add_legal_section(
        "libmpv, FFmpeg",
        None,
        gtk::License::Lgpl21,
        Some(&tr("Playback and decoding. Depending on how your distribution builds them, they are LGPL or GPL software; AniWay is GPL so that either works.")),
    );
    dialog.add_legal_section("GTK, libadwaita", None, gtk::License::Lgpl21, None);
    dialog.add_legal_section(
        "Space Grotesk, DM Sans, JetBrains Mono",
        Some("© The Space Grotesk, DM Sans and JetBrains Mono Project Authors"),
        gtk::License::Custom,
        Some(&tr("Bundled fonts, under the SIL Open Font License 1.1.")),
    );
    dialog.add_acknowledgement_section(
        Some(&tr("Built on")),
        &[
            "GTK https://gtk.org",
            "libadwaita https://gnome.pages.gitlab.gnome.org/libadwaita/",
            "mpv https://mpv.io",
            "FFmpeg https://ffmpeg.org",
        ],
    );
    // The dialog fills its header when shown.
    dialog.connect_map(|d| {
        link_label(d.upcast_ref(), REPO_SHORT, REPO);
    });
    dialog
}

/// Turns the label showing `text` under `root` into a link; the dialog offers no slot under its title for one.
fn link_label(root: &gtk::Widget, text: &str, uri: &str) -> bool {
    if let Some(label) = root.downcast_ref::<gtk::Label>().filter(|l| l.label() == text) {
        label.set_markup(&format!("<a href=\"{uri}\">{}</a>", glib::markup_escape_text(text)));
        return true;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if link_label(&c, text, uri) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// Keys of the map, under it: what a first-time user needs to move stickers precisely.
fn hint_bar() -> gtk::FlowBox {
    let bar = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(4)
        .halign(gtk::Align::Center)
        .column_spacing(18)
        .css_classes(["hintbar"])
        .build();
    let item = |lead: gtk::Widget, text: String| {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        b.append(&lead);
        b.append(&gtk::Label::new(Some(&text)));
        bar.append(&b);
    };
    let keycap = |k: &str| gtk::Label::builder().label(k).css_classes(["keycap"]).valign(gtk::Align::Center).build();
    item(gtk::Image::from_icon_name("aniway-placement-symbolic").upcast(), tr("Drag or use the arrow keys"));
    item(keycap("Shift").upcast(), tr("10 px steps"));
    item(keycap("Alt").upcast(), tr("no snapping"));
    item(keycap("Tab").upcast(), tr("next overlay"));
    bar
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let ctl = Ctl::new();
    let list = List::new(ctl.clone());
    let map = MonitorMap::new(ctl.clone());
    let inspector = Inspector::new(ctl.clone());

    let centre = gtk::Box::new(gtk::Orientation::Vertical, 0);
    map.set_vexpand(true);
    let map_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    map_page.append(&map);
    map_page.append(&hint_bar());
    let library = crate::library::Library::new(ctl.clone());
    let mode_pages = gtk::Stack::new();
    mode_pages.add_named(&map_page, Some("map"));
    mode_pages.add_named(&library.widget, Some("library"));
    mode_pages.set_vexpand(true);
    let mode = adw::ToggleGroup::builder().halign(gtk::Align::Center).margin_top(14).margin_bottom(6).build();
    let map_button = adw::Toggle::builder().label(tr("Map")).name("map").build();
    let library_button = adw::Toggle::builder().label(tr("Library")).name("library").build();
    mode.add(map_button);
    mode.add(library_button);
    centre.append(&mode);
    centre.append(&mode_pages);
    centre.add_css_class("centre");
    mode.connect_active_notify(glib::clone!(
        #[weak]
        mode_pages,
        #[strong]
        library,
        move |group| {
            if group.active_name().as_deref() == Some("library") {
                mode_pages.set_visible_child_name("library");
                library.refresh();
            } else {
                mode_pages.set_visible_child_name("map");
            }
        }
    ));
    let inner = adw::OverlaySplitView::builder()
        .content(&centre)
        .sidebar(&inspector.widget)
        .sidebar_position(gtk::PackType::End)
        .build();
    // Wide enough that Spanish row titles ("Desplazamiento X", "Opacidad") never hyphenate beside their controls.
    inner.set_min_sidebar_width(330.0);
    inner.set_max_sidebar_width(390.0);
    inner.set_sidebar_width_fraction(0.3);
    let outer = adw::OverlaySplitView::builder().content(&inner).sidebar(&list.widget).build();
    outer.set_min_sidebar_width(260.0);
    outer.set_max_sidebar_width(320.0);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.append(&outer);
    outer.set_vexpand(true);

    let start = gtk::Button::builder()
        .label(tr("Start"))
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::Center)
        .build();
    start.connect_clicked(|_| start_daemon());
    let down = adw::StatusPage::builder()
        .icon_name(crate::theme::logo())
        .title(tr("AniWay is not running"))
        .description(tr("The daemon draws the overlays; this editor only controls it."))
        .child(&start)
        .build();
    // The logo comes in a light and a dark drawing; follow the theme.
    adw::StyleManager::default().connect_dark_notify(glib::clone!(
        #[weak]
        down,
        move |_| down.set_icon_name(Some(crate::theme::logo()))
    ));
    let pages = gtk::Stack::new();
    pages.add_named(&main, Some("main"));
    pages.add_named(&down, Some("down"));
    ctl.toasts.set_child(Some(&pages));

    // Icon plus a label that the narrow breakpoint hides.
    let icon_label = |icon: &str, text: &str| {
        let (row, label) = (gtk::Box::new(gtk::Orientation::Horizontal, 6), gtk::Label::new(Some(text)));
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&label);
        (row, label)
    };
    let (add_child, add_label) = icon_label("list-add-symbolic", &tr("Add"));
    let add = gtk::Button::builder()
        .child(&add_child)
        .css_classes(["suggested-action"])
        .tooltip_text(tr("Add media (several at once)"))
        .build();
    let (scene_child, scene_label) = icon_label("document-new-symbolic", &tr("New scene"));
    let new_scene = gtk::Button::builder()
        .child(&scene_child)
        .action_name("win.new-scene")
        .tooltip_text(tr("New empty scene; the current one stays as it is"))
        .css_classes(["secondary-action"])
        .build();
    let (pause_child, pause_label) = icon_label("media-playback-pause-symbolic", &tr("Pause all"));
    let pause =
        gtk::ToggleButton::builder().child(&pause_child).tooltip_text(tr("Pause all")).css_classes(["flat"]).build();
    pause.update_property(&[gtk::accessible::Property::Label(&tr("Pause all"))]);
    let pause_icon = pause_child.first_child().and_downcast::<gtk::Image>().expect("icon_label starts with the icon");
    let hide_menu = gio::Menu::new();
    for (label, secs) in
        [(tr("Hide until shown"), 0), (tr("Hide for 15 minutes"), 15 * 60), (tr("Hide for 1 hour"), 3600)]
    {
        hide_menu.append(Some(&label), Some(&format!("win.hide(uint32 {secs})")));
    }
    let (hide_child, hide_label) = icon_label("view-conceal-symbolic", &tr("Hide 15 min"));
    let hide = adw::SplitButton::builder().child(&hide_child).menu_model(&hide_menu).build();
    let main_menu = gio::Menu::new();
    main_menu.append(Some(&tr("Preferences")), Some("win.preferences"));
    main_menu.append(Some(&tr("Keyboard shortcuts")), Some("win.shortcuts"));
    main_menu.append(Some(&tr("Check for updates")), Some("win.update"));
    main_menu.append(Some(&tr("About AniWay")), Some("win.about"));
    let menu = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&main_menu).primary(true).build();
    menu.update_property(&[gtk::accessible::Property::Label(&tr("Main menu"))]);
    let title = adw::WindowTitle::new("AniWay", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    // Panel toggles only show while the matching pane has collapsed into an overlay.
    let toggle = |icon: &str, label: String, view: &adw::OverlaySplitView| {
        let b = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(&label).build();
        b.update_property(&[gtk::accessible::Property::Label(&label)]);
        view.bind_property("collapsed", &b, "visible").sync_create().build();
        view.bind_property("show-sidebar", &b, "active").sync_create().bidirectional().build();
        b
    };
    header.pack_start(&toggle("sidebar-show-symbolic", tr("Overlay list"), &outer));
    header.pack_start(&add);
    header.pack_start(&new_scene);
    header.pack_end(&menu);
    header.pack_end(&toggle("sidebar-show-right-symbolic", tr("Inspector"), &inner));
    header.pack_end(&hide);
    header.pack_end(&pause);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&ctl.toasts));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(1280)
        .default_height(800)
        .content(&toolbar)
        .build();
    window.add_css_class("editor");
    // Small enough to stay whole when tiled into half (or a third) of a screen.
    window.set_size_request(360, 420);
    let scene = SceneMenu::new(&ctl, &window);
    header.pack_start(&scene.button);
    let prefs = Prefs::new(ctl.clone());
    let dialog = |action: &str, f: Rc<dyn Fn(&adw::ApplicationWindow)>| {
        let entry = gio::ActionEntry::builder(action).activate(move |w: &adw::ApplicationWindow, _, _| f(w)).build();
        window.add_action_entries([entry]);
    };
    dialog("preferences", Rc::new(move |w| prefs.present(w)));
    dialog("shortcuts", Rc::new(|w| shortcuts().present(Some(w))));
    dialog("about", Rc::new(|w| about().present(Some(w))));
    dialog(
        "update",
        Rc::new({
            let ctl = ctl.clone();
            move |w| crate::update::check(&ctl, w)
        }),
    );
    crate::update::check_quietly(&ctl);
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    app.set_accels_for_action("win.shortcuts", &["<Control>question"]);
    app.set_accels_for_action("win.add", &["<Control>o"]);
    app.set_accels_for_action("win.new-scene", &["<Control>n"]);
    dialog(
        "new-scene",
        Rc::new({
            let ctl = ctl.clone();
            move |w| crate::scenes::new_scene(&ctl, w)
        }),
    );
    dialog(
        "add",
        Rc::new({
            let ctl = ctl.clone();
            move |w| add_files(&ctl, w)
        }),
    );
    let secs = gio::ActionEntry::builder("hide")
        .parameter_type(Some(glib::VariantTy::UINT32))
        .activate(glib::clone!(
            #[strong]
            ctl,
            move |_: &adw::ApplicationWindow, _, secs| {
                let secs = secs.and_then(|v| v.get::<u32>()).unwrap_or(0);
                ctl.send(Request::Hide { id: None, for_secs: Some(u64::from(secs)).filter(|s| *s > 0) });
            }
        ))
        .build();
    window.add_action_entries([secs]);
    // A breakpoint replaces the earlier ones, so the narrower one repeats the inspector setters.
    for (width, views) in [(1100, vec![&inner]), (900, vec![&inner, &outer])] {
        let condition = adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            f64::from(width),
            adw::LengthUnit::Px,
        );
        let breakpoint = adw::Breakpoint::new(condition);
        for view in views {
            breakpoint.add_setter(view, "collapsed", Some(&true.to_value()));
            breakpoint.add_setter(view, "show-sidebar", Some(&false.to_value()));
        }
        if width == 900 {
            let hidden = false.to_value();
            breakpoint.add_setter(&title, "visible", Some(&hidden));
            breakpoint.add_setter(&pause_label, "visible", Some(&hidden));
            breakpoint.add_setter(&add_label, "visible", Some(&hidden));
            breakpoint.add_setter(&scene_label, "visible", Some(&hidden));
            breakpoint.add_setter(&hide_label, "visible", Some(&hidden));
        }
        window.add_breakpoint(breakpoint);
    }
    // Files can be dropped anywhere in the window; the zone in the list lights up meanwhile. MOVE must be accepted
    // too: Hyprland announces the offer as a move before the client states its preference, and a copy-only target
    // then rejects every drop. Answering COPY makes the source keep its file.
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let zone = list.drop.clone();
    drop.connect_enter(move |_, _, _| {
        zone.set_active(true);
        gdk::DragAction::COPY
    });
    let zone = list.drop.clone();
    drop.connect_leave(move |_| zone.set_active(false));
    let zone = list.drop.clone();
    drop.connect_drop(glib::clone!(
        #[strong]
        ctl,
        move |_, value, _, _| {
            zone.set_active(false);
            value.get::<gdk::FileList>().map(|files| ctl.add_files(&files)).is_ok()
        }
    ));
    window.add_controller(drop);

    add.connect_clicked(glib::clone!(
        #[strong]
        ctl,
        #[weak]
        window,
        move |_| add_files(&ctl, &window)
    ));
    let syncing = Rc::new(Cell::new(false));
    pause.connect_toggled(glib::clone!(
        #[strong]
        ctl,
        #[strong]
        syncing,
        move |b| {
            if !syncing.get() {
                ctl.send(if b.is_active() { Request::Pause { id: None } } else { Request::Resume { id: None } });
            }
        }
    ));
    hide.connect_clicked(glib::clone!(
        #[strong]
        ctl,
        move |_| {
            let hidden = ctl.state.borrow().as_ref().is_some_and(|s| s.hidden || s.snooze_secs_left.is_some());
            ctl.send(if hidden {
                Request::Show { id: None }
            } else {
                Request::Hide { id: None, for_secs: Some(SNOOZE_SECS.into()) }
            });
        }
    ));
    // Label of the hide button; the countdown runs locally between state events.
    let snooze_end: Rc<Cell<Option<Instant>>> = Rc::default();
    let seen_left = Cell::new(None);
    // xgettext skips strings inside macro calls.
    let (show_left, show) = (tr("Show ({} min left)"), tr("Show"));
    let render_hide = Rc::new(glib::clone!(
        #[strong]
        ctl,
        #[strong]
        snooze_end,
        #[weak]
        hide_label,
        #[weak]
        hide,
        move || {
            let Some(s) = ctl.state.borrow().as_ref().map(|s| (s.hidden, s.snooze_secs_left)) else { return };
            if s.1 != seen_left.replace(s.1) {
                snooze_end.set(s.1.map(|left| Instant::now() + Duration::from_secs(left)));
            }
            let left = snooze_end.get().map(|end| end.saturating_duration_since(Instant::now()).as_secs());
            let label = match (s.0, left) {
                (_, Some(left)) => show_left.replace("{}", &left.div_ceil(60).to_string()),
                (true, None) => show.clone(),
                _ => tr("Hide 15 min"),
            };
            hide_label.set_label(&label);
            hide.set_tooltip_text(Some(&label));
        }
    ));
    glib::timeout_add_seconds_local(20, {
        let render = render_hide.clone();
        move || {
            render();
            glib::ControlFlow::Continue
        }
    });

    let last_selected = RefCell::new(None);
    ctl.on_change({
        let (ctl, map) = (ctl.clone(), map.clone());
        move || {
            list.update();
            inspector.update();
            map.refresh();
            let selected = ctl.selected.borrow().clone();
            if selected.is_some() && *last_selected.borrow() != selected && inner.is_collapsed() {
                inner.set_show_sidebar(true);
            }
            *last_selected.borrow_mut() = selected;
            if let Some(s) = ctl.state.borrow().as_ref() {
                scene.set_scene(&s.scene);
                syncing.set(true);
                pause.set_active(s.paused);
                syncing.set(false);
                let (icon, text) = if s.paused {
                    ("media-playback-start-symbolic", tr("Resume all"))
                } else {
                    ("media-playback-pause-symbolic", tr("Pause all"))
                };
                pause_icon.set_icon_name(Some(icon));
                pause_label.set_text(&text);
                pause.set_tooltip_text(Some(&text));
                pause.update_property(&[gtk::accessible::Property::Label(&text)]);
            }
            render_hide();
        }
    });

    let events = client::subscribe();
    let app = app.clone();
    glib::spawn_future_local(async move {
        while let Ok(event) = events.recv().await {
            match event {
                client::Event::State(state) => {
                    header.set_sensitive(true);
                    outer.set_sensitive(true);
                    pages.set_visible_child_name("main");
                    ctl.set_state(Some(*state));
                }
                client::Event::Down => {
                    header.set_sensitive(false);
                    outer.set_sensitive(false);
                    ctl.set_state(None);
                    pages.set_visible_child_name("down");
                }
                client::Event::Quit => app.quit(),
            }
        }
    });
    window
}
