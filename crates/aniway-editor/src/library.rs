//! Media library: folder scan, animated grid, add popover and drag source.

use crate::client::Ctl;
use crate::media::{self, PREVIEW_FPS};
use crate::tr;
use adw::prelude::*;
use aniway_proto::paths::{extension, home, is_media_extension};
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const MAX_DEPTH: usize = 4;

/// Recursively scans configured folders for supported media.
pub fn scan(folders: &[PathBuf]) -> Vec<String> {
    let mut found = Vec::new();
    for folder in folders {
        visit(folder, 0, &mut found);
    }
    found.sort_by_key(|p| p.to_ascii_lowercase());
    found
}

fn visit(path: &Path, depth: usize, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && depth < MAX_DEPTH {
            visit(&path, depth + 1, out);
        } else if path.is_file() {
            let text = path.to_string_lossy();
            if is_media_extension(&extension(&text)) {
                out.push(text.into_owned());
            }
        }
    }
}

pub async fn scan_async(folders: Vec<PathBuf>) -> Vec<String> {
    gio::spawn_blocking(move || scan(&folders)).await.unwrap_or_default()
}

/// Opens the add popover after loading the current scene list.
fn add_popover(cell: &gtk::Button, path: String, ctl: &Rc<Ctl>) {
    let cell = cell.clone();
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        let data = crate::client::call(aniway_proto::ipc::Request::SceneList).await.unwrap_or_default();
        let scenes: Vec<String> =
            data["scenes"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let active = ctl.state.borrow().as_ref().map(|s| s.scene.clone()).unwrap_or_default();
        let pop = gtk::Popover::new();
        pop.set_parent(&cell);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(10)
            .margin_end(10)
            .build();
        let scene_refs: Vec<_> = scenes.iter().map(String::as_str).collect();
        let scene_dropdown = gtk::DropDown::from_strings(&scene_refs);
        let monitor_dropdown = gtk::DropDown::from_strings(&[]);
        content.append(&gtk::Label::builder().label(tr("Scene")).xalign(0.0).build());
        content.append(&scene_dropdown);
        content.append(&gtk::Label::builder().label(tr("Monitor")).xalign(0.0).build());
        content.append(&monitor_dropdown);
        let add = gtk::Button::builder().label(tr("Add")).css_classes(["suggested-action"]).build();
        content.append(&add);
        pop.set_child(Some(&content));
        if let Some(i) = scenes.iter().position(|n| n == &active) {
            scene_dropdown.set_selected(i as u32);
        }
        if let Some(state) = ctl.state.borrow().as_ref() {
            let names: Vec<_> = state.outputs.iter().map(|o| o.name.as_str()).collect();
            monitor_dropdown.set_model(Some(&gtk::StringList::new(&names)));
            if let Some(focus) = ctl.focus.borrow().as_ref() {
                if let Some(i) = names.iter().position(|n| *n == focus) {
                    monitor_dropdown.set_selected(i as u32);
                }
            }
        }
        let scenes_for_add = scene_dropdown.clone();
        let monitors_for_add = monitor_dropdown.clone();
        let path_for_add = path.clone();
        let ctl_for_add = ctl.clone();
        let active_for_add = active.clone();
        add.connect_clicked(move |_| {
            let scene = scenes_for_add
                .selected_item()
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                .map(|o| o.string().to_string())
                .unwrap_or_else(|| active_for_add.clone());
            let output = monitors_for_add
                .selected_item()
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                .map(|o| o.string().to_string());
            ctl_for_add.add_library(path_for_add.clone(), scene, output);
        });
        let weak_pop = pop.downgrade();
        pop.connect_closed(move |_| {
            if let Some(pop) = weak_pop.upgrade() {
                pop.unparent();
            }
        });
        pop.popup();
    });
}

#[derive(Default)]
struct CellState {
    path: Option<String>,
    frames: Option<Rc<Vec<gtk::gdk::Texture>>>,
    index: usize,
    image: Option<glib::WeakRef<gtk::Image>>,
    timer: Option<glib::SourceId>,
}

fn stop_timer(state: &mut CellState) {
    if let Some(source) = state.timer.take() {
        source.remove();
    }
}

fn start_timer(cell: &Rc<RefCell<CellState>>) {
    let mut state = cell.borrow_mut();
    if state.timer.is_some() || state.frames.as_ref().is_none_or(|frames| frames.is_empty()) {
        return;
    }
    let Some(image) = state.image.as_ref().and_then(glib::WeakRef::upgrade) else { return };
    let weak_image = image.downgrade();
    let cell_for_timer = cell.clone();
    let source = glib::timeout_add_local(std::time::Duration::from_millis(1000 / u64::from(PREVIEW_FPS)), move || {
        let Some(image) = weak_image.upgrade() else { return glib::ControlFlow::Break };
        let mut state = cell_for_timer.borrow_mut();
        if !image.is_mapped() {
            state.timer = None;
            return glib::ControlFlow::Break;
        }
        let Some(frames) = state.frames.clone() else { return glib::ControlFlow::Break };
        state.index = (state.index + 1) % frames.len();
        image.set_paintable(frames.get(state.index));
        glib::ControlFlow::Continue
    });
    state.timer = Some(source);
}

struct PreviewJob {
    path: String,
    cell: Rc<RefCell<CellState>>,
}

type CellEntry = (gtk::Button, Rc<RefCell<CellState>>);

/// Media library grid backed by the configured folders.
pub struct Library {
    pub widget: gtk::Box,
    store: gio::ListStore,
    count: gtk::Label,
    folder: gtk::Label,
    body: gtk::Stack,
    _preview_tx: async_channel::Sender<PreviewJob>,
}

impl Library {
    pub fn new(ctl: Rc<Ctl>) -> Rc<Self> {
        let store = gio::ListStore::new::<gtk::StringObject>();
        let selection = gtk::NoSelection::new(Some(store.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let (preview_tx, preview_rx) = async_channel::unbounded::<PreviewJob>();
        glib::spawn_future_local(async move {
            while let Ok(job) = preview_rx.recv().await {
                // Fast scrolling queues cells that were recycled since; skip them before running ffmpeg.
                if job.cell.borrow().path.as_deref() != Some(job.path.as_str()) {
                    continue;
                }
                let mut frames = media::preview_frames(job.path.clone(), 128).await;
                if frames.is_empty() {
                    // ffmpeg's fps filter yields no frame for a still image: show the picture itself.
                    let path = job.path.clone();
                    let still = gio::spawn_blocking(move || gtk::gdk::Texture::from_filename(path).ok()).await;
                    frames.extend(still.ok().flatten());
                }
                let mut state = job.cell.borrow_mut();
                if state.path.as_deref() != Some(job.path.as_str()) {
                    continue;
                }
                let Some(image) = state.image.as_ref().and_then(glib::WeakRef::upgrade) else { continue };
                if let Some(first) = frames.first().cloned() {
                    let frames = Rc::new(frames);
                    state.index = 0;
                    state.frames = Some(frames.clone());
                    image.set_paintable(Some(&first));
                    drop(state);
                    if image.is_mapped() {
                        start_timer(&job.cell);
                    }
                }
            }
        });
        let cells: Rc<RefCell<Vec<CellEntry>>> = Rc::default();
        let ctl_for_setup = ctl.clone();
        let cells_for_setup = cells.clone();
        factory.connect_setup(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let button = gtk::Button::builder().css_classes(["library-cell", "flat"]).build();
            let image = gtk::Image::builder()
                .pixel_size(128)
                .width_request(128)
                .height_request(128)
                .icon_name("image-x-generic-symbolic")
                .build();
            let label = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(18).build();
            let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
            content.append(&image);
            content.append(&label);
            button.set_child(Some(&content));
            let c = ctl_for_setup.clone();
            button.connect_clicked(move |b| {
                let path = b.tooltip_text().map(|s| s.to_string()).unwrap_or_default();
                if !path.is_empty() {
                    add_popover(b, path, &c);
                }
            });
            let cell = Rc::new(RefCell::new(CellState::default()));
            let map_cell = cell.clone();
            image.connect_map(move |_| start_timer(&map_cell));
            let unmap_cell = cell.clone();
            image.connect_unmap(move |_| stop_timer(&mut unmap_cell.borrow_mut()));
            let source = gtk::DragSource::builder().actions(gtk::gdk::DragAction::COPY).build();
            let source_button = button.clone();
            source.connect_prepare(move |_, _, _| {
                let path = source_button.tooltip_text()?.to_string();
                let file = gio::File::for_path(path);
                let files = gtk::gdk::FileList::from_array(&[file]);
                Some(gtk::gdk::ContentProvider::for_value(&files.to_value()))
            });
            button.add_controller(source);
            cells_for_setup.borrow_mut().push((button.clone(), cell));
            item.set_child(Some(&button));
        });
        let cells_for_bind = cells.clone();
        let tx_for_bind = preview_tx.clone();
        factory.connect_bind(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(button) = item.child().and_then(|w| w.downcast::<gtk::Button>().ok()) else { return };
            let Some(obj) = item.item().and_then(|o| o.downcast::<gtk::StringObject>().ok()) else { return };
            let path = obj.string().to_string();
            let Some(cell) = cells_for_bind
                .borrow()
                .iter()
                .find(|(candidate, _)| candidate == &button)
                .map(|(_, cell)| cell.clone())
            else {
                return;
            };
            let Some(content) = button.child().and_then(|w| w.downcast::<gtk::Box>().ok()) else { return };
            let Some(image) = content.first_child().and_then(|w| w.downcast::<gtk::Image>().ok()) else { return };
            let Some(label) = image.next_sibling().and_then(|w| w.downcast::<gtk::Label>().ok()) else { return };
            let mut state = cell.borrow_mut();
            stop_timer(&mut state);
            state.path = Some(path.clone());
            state.frames = None;
            state.index = 0;
            state.image = Some(image.downgrade());
            drop(state);
            button.set_tooltip_text(Some(&path));
            label.set_text(path.rsplit('/').next().unwrap_or(&path));
            image.set_icon_name(Some("image-x-generic-symbolic"));
            let _ = tx_for_bind.try_send(PreviewJob { path, cell });
        });
        let cells_for_unbind = cells.clone();
        factory.connect_unbind(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else { return };
            let Some(button) = item.child().and_then(|w| w.downcast::<gtk::Button>().ok()) else { return };
            if let Some(cell) = cells_for_unbind
                .borrow()
                .iter()
                .find(|(candidate, _)| candidate == &button)
                .map(|(_, cell)| cell.clone())
            {
                let mut state = cell.borrow_mut();
                stop_timer(&mut state);
                state.path = None;
                state.frames = None;
                state.image = None;
            }
        });
        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.set_min_columns(2);
        grid.set_max_columns(8);
        grid.set_single_click_activate(false);
        grid.set_hexpand(true);
        grid.set_vexpand(true);
        let grid_drop = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        grid_drop.connect_drop(|_, _, _, _| true);
        grid.add_controller(grid_drop);
        let count = gtk::Label::builder().css_classes(["dim-label"]).build();
        let folder = gtk::Label::builder()
            .css_classes(["dim-label", "caption"])
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .hexpand(true)
            .xalign(1.0)
            .build();
        let change = gtk::Button::builder().icon_name("folder-open-symbolic").tooltip_text(tr("Change folder")).build();
        let refresh =
            gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text(tr("Refresh library")).build();
        let header =
            gtk::Box::builder().spacing(8).margin_start(12).margin_end(12).margin_top(10).margin_bottom(8).build();
        header.append(&gtk::Label::builder().label(tr("Library")).css_classes(["title-3"]).build());
        header.append(&count);
        header.append(&folder);
        header.append(&change);
        header.append(&refresh);
        let empty = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        empty.append(&gtk::Image::from_icon_name("folder-music-symbolic"));
        empty.append(&gtk::Label::builder().label(tr("Choose a folder to browse media")).build());
        let choose = gtk::Button::builder().label(tr("Choose a folder")).css_classes(["suggested-action"]).build();
        empty.append(&choose);
        let body = gtk::Stack::new();
        body.add_named(&grid, Some("grid"));
        body.add_named(&empty, Some("empty"));
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&header);
        widget.append(&body);
        let this = Rc::new(Self { widget, store, count, folder, body, _preview_tx: preview_tx });
        let weak = Rc::downgrade(&this);
        refresh.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.refresh();
            }
        });
        // Both buttons replace the scanned folders with the chosen one; Preferences can list several.
        let weak = Rc::downgrade(&this);
        let pick = move |_: &gtk::Button| {
            let (Some(this), ctl) = (weak.upgrade(), ctl.clone()) else { return };
            gtk::FileDialog::new().select_folder(None::<&gtk::Window>, gio::Cancellable::NONE, move |r| {
                let Some(path) = r.ok().and_then(|folder| folder.path()) else { return };
                match crate::settings::set_library_folder(&path) {
                    Ok(()) => this.refresh(),
                    Err(error) => ctl.toast(&error.to_string()),
                }
            });
        };
        choose.connect_clicked(pick.clone());
        change.connect_clicked(pick);
        this.refresh();
        this
    }

    /// Rescans effective folders and updates the grid or empty state.
    pub fn refresh(&self) {
        let store = self.store.clone();
        let count = self.count.clone();
        let body = self.body.clone();
        let folders = crate::settings::library_folders();
        let shown: Vec<String> = folders.iter().map(|f| tilde(f)).collect();
        self.folder.set_text(&match shown.len() {
            0 => tr("No folder"),
            1 => shown[0].clone(),
            n => format!("{} (+{})", shown[0], n - 1),
        });
        self.folder.set_tooltip_text(Some(&shown.join("\n")));
        glib::spawn_future_local(async move {
            let files = scan_async(folders).await;
            store.remove_all();
            for path in &files {
                store.append(&gtk::StringObject::new(path));
            }
            count.set_text(&tr("{} items").replace("{}", &files.len().to_string()));
            body.set_visible_child_name(if files.is_empty() { "empty" } else { "grid" });
        });
    }
}

/// The path with the home directory written as `~`.
fn tilde(path: &Path) -> String {
    path.strip_prefix(home()).map_or_else(|_| path.display().to_string(), |rest| format!("~/{}", rest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_supported_non_hidden_files_with_depth_limit() {
        let dir = std::env::temp_dir().join(format!("aw-library-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.gif"), []).unwrap();
        fs::write(dir.join("no.txt"), []).unwrap();
        fs::create_dir(dir.join(".hidden")).unwrap();
        fs::write(dir.join(".hidden/a.gif"), []).unwrap();
        let mut deep = dir.clone();
        for i in 0..6 {
            deep.push(format!("d{i}"));
            fs::create_dir(&deep).unwrap();
        }
        fs::write(deep.join("too-deep.gif"), []).unwrap();
        let files = scan(std::slice::from_ref(&dir));
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("b.gif"));
        let _ = fs::remove_dir_all(dir);
    }
}
