//! IPC with the daemon plus the shared controller: the latest state is the only source of truth.

use crate::media::{self, Media};
use crate::tr;
use aniway_proto::geometry::Size;
use aniway_proto::ipc::{encode_request, Request, Response, StateData};
use aniway_proto::model::{Overlay, OverlayPatch};
use aniway_proto::paths;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::{Duration, Instant};

const RETRY: Duration = Duration::from_secs(2);
const TRANSIENT_GAP: Duration = Duration::from_millis(33);
const COMMIT_DELAY: Duration = Duration::from_millis(400);
/// How long a committed local value keeps winning while waiting for the state event that confirms it.
const SETTLE: Duration = Duration::from_secs(1);

fn connect(request: &Request) -> anyhow::Result<(BufReader<UnixStream>, Response)> {
    let mut stream = UnixStream::connect(paths::socket_path())?;
    writeln!(stream, "{}", encode_request(request))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok((reader, serde_json::from_str(&line)?))
}

/// One blocking request; the daemon's error text becomes the `Err`.
fn request(request: &Request) -> anyhow::Result<Value> {
    let (_, response) = connect(request)?;
    anyhow::ensure!(response.ok, "{}", response.error.unwrap_or_default());
    Ok(response.data.unwrap_or(Value::Null))
}

pub async fn call(req: Request) -> anyhow::Result<Value> {
    gio::spawn_blocking(move || request(&req)).await.map_err(|_| anyhow::anyhow!("request thread panicked"))?
}

/// Events from the daemon subscription: state updates, connection loss, or a clean daemon quit.
pub enum Event {
    State(Box<StateData>),
    Down,
    Quit,
}

/// Streams daemon events and reconnects forever after a dropped connection.
pub fn subscribe() -> async_channel::Receiver<Event> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || loop {
        if let Ok((reader, response)) = connect(&Request::Subscribe) {
            if response.ok {
                for line in reader.lines().map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    let event = match v.get("event").and_then(Value::as_str) {
                        Some("quit") => Some(Event::Quit),
                        Some("state") => serde_json::from_value::<StateData>(v["data"].clone())
                            .ok()
                            .map(|state| Event::State(Box::new(state))),
                        _ => None,
                    };
                    if let Some(event) = event {
                        let quit = matches!(event, Event::Quit);
                        if tx.send_blocking(event).is_err() || quit {
                            return;
                        }
                    }
                }
            }
        }
        if tx.send_blocking(Event::Down).is_err() {
            return;
        }
        std::thread::sleep(RETRY);
    });
    rx
}

struct Pending {
    id: String,
    patch: serde_json::Map<String, Value>,
    generation: u64,
    /// Set once the saving `set` was sent: the value still wins until confirmed or this instant.
    settle_until: Option<Instant>,
}

/// Shared by every view: latest state, selection and the write path.
pub struct Ctl {
    pub state: RefCell<Option<StateData>>,
    pub selected: RefCell<Option<String>>,
    pub focus: RefCell<Option<String>>,
    /// While true (a drag is in progress) the delayed commit is suspended; releasing commits explicitly.
    pub holding: Cell<bool>,
    pub toasts: adw::ToastOverlay,
    pub media: RefCell<HashMap<String, Media>>,
    pending: RefCell<Option<Pending>>,
    last_transient: Cell<Option<Instant>>,
    generation: Cell<u64>,
    on_change: RefCell<Vec<Box<dyn Fn()>>>,
}

impl Ctl {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::default(),
            selected: RefCell::default(),
            focus: RefCell::default(),
            holding: Cell::default(),
            toasts: adw::ToastOverlay::new(),
            media: RefCell::default(),
            pending: RefCell::default(),
            last_transient: Cell::default(),
            generation: Cell::default(),
            on_change: RefCell::default(),
        })
    }

    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.on_change.borrow_mut().push(Box::new(f));
    }

    fn changed(&self) {
        self.on_change.borrow().iter().for_each(|f| f());
    }

    /// True while a local edit has not been committed yet: views of that value must not be overwritten.
    pub fn editing(&self) -> bool {
        self.pending.borrow().as_ref().is_some_and(|p| p.settle_until.is_none())
    }

    /// The state with the local, unconfirmed edit applied (so views never jump back during or after a drag) and the
    /// ffprobe size standing in for a missing `media`.
    pub fn view(&self) -> Option<StateData> {
        let mut state = self.state.borrow().clone()?;
        for ov in &mut state.overlays {
            ov.media = ov.media.or_else(|| self.media_size(&ov.config.path));
        }
        let pending = self.pending.borrow();
        let local = pending.as_ref().filter(|p| p.settle_until.is_none_or(|t| t > Instant::now()));
        let ov = local.and_then(|l| state.overlays.iter_mut().find(|o| o.config.id == l.id));
        if let (Some(local), Some(ov)) = (local, ov) {
            if let Ok(Value::Object(mut cfg)) = serde_json::to_value(&ov.config) {
                cfg.extend(local.patch.clone());
                ov.config = serde_json::from_value(Value::Object(cfg)).unwrap_or_else(|_| ov.config.clone());
            }
        }
        Some(state)
    }

    pub fn set_state(self: &Rc<Self>, state: Option<StateData>) {
        if let Some(s) = &state {
            let known = |n: &Option<String>| n.as_ref().is_some_and(|n| s.outputs.iter().any(|o| &o.name == n));
            if !known(&self.focus.borrow()) {
                *self.focus.borrow_mut() = s.outputs.first().map(|o| o.name.clone());
            }
            if !self.selected.borrow().as_ref().is_some_and(|id| s.overlays.iter().any(|o| &o.config.id == id)) {
                *self.selected.borrow_mut() = None;
            }
            let confirmed = |p: &Pending| {
                p.settle_until.is_some()
                    && s.overlays.iter().find(|o| o.config.id == p.id).is_none_or(|o| {
                        let cfg = serde_json::to_value(&o.config).unwrap_or_default();
                        p.patch.iter().all(|(k, v)| cfg.get(k) == Some(v))
                    })
            };
            if self.pending.borrow().as_ref().is_some_and(confirmed) {
                self.pending.borrow_mut().take();
            }
            for ov in &s.overlays {
                self.load_media(&ov.config.path);
            }
        }
        *self.state.borrow_mut() = state;
        self.changed();
    }

    fn load_media(self: &Rc<Self>, path: &str) {
        if !self.media.borrow().contains_key(path) {
            self.media.borrow_mut().insert(path.to_string(), Media::default());
            let (ctl, path) = (self.clone(), path.to_string());
            glib::spawn_future_local(async move {
                let loaded = media::load(path.clone()).await;
                ctl.media.borrow_mut().insert(path, loaded);
                ctl.changed();
            });
        }
    }

    /// ffprobe size of an overlay's file, the fallback when the daemon reports none.
    pub fn media_size(&self, path: &str) -> Option<Size> {
        self.media.borrow().get(path).and_then(|m| m.size)
    }

    /// Selects an overlay and focuses the output it lives on, so the map always shows the selection.
    pub fn select(&self, id: Option<String>) {
        let output = self.state.borrow().as_ref().zip(id.as_ref()).and_then(|(s, id)| {
            let ov = s.overlays.iter().find(|o| &o.config.id == id)?;
            ov.config.output.clone().or_else(|| s.outputs.first().map(|o| o.name.clone()))
        });
        if output.is_some() {
            *self.focus.borrow_mut() = output;
        }
        *self.selected.borrow_mut() = id;
        self.changed();
    }

    pub fn focus_output(&self, name: &str) {
        *self.focus.borrow_mut() = Some(name.to_string());
        self.changed();
    }

    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// Fire-and-forget request; failures show a toast.
    pub fn send(self: &Rc<Self>, req: Request) {
        let ctl = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = call(req).await {
                ctl.toast(&e.to_string());
            }
        });
    }

    /// Live edit of the selected overlay: a transient `set` at most every 33 ms, then one normal `set` 400 ms
    /// after the last change so the scene file is written once.
    pub fn edit(self: &Rc<Self>, patch: OverlayPatch) {
        let Some(id) = self.selected.borrow().clone() else { return };
        if self.pending.borrow().as_ref().is_some_and(|p| p.id != id && p.settle_until.is_none()) {
            self.commit();
        }
        let mut pending = self.pending.borrow_mut().take().filter(|p| p.id == id).unwrap_or(Pending {
            id: id.clone(),
            patch: Default::default(),
            generation: 0,
            settle_until: None,
        });
        if let Ok(Value::Object(fields)) = serde_json::to_value(&patch) {
            pending.patch.extend(fields.into_iter().filter(|(_, v)| !v.is_null()));
        }
        pending.settle_until = None;
        let now = Instant::now();
        if self.last_transient.get().is_none_or(|t| now - t >= TRANSIENT_GAP) {
            self.last_transient.set(Some(now));
            let patch = serde_json::from_value(Value::Object(pending.patch.clone())).unwrap_or_default();
            self.send(Request::Set { id, patch, transient: true });
        }
        pending.generation = self.generation.get() + 1;
        self.generation.set(pending.generation);
        let (ctl, generation) = (self.clone(), pending.generation);
        glib::timeout_add_local_once(COMMIT_DELAY, move || {
            if ctl.generation.get() == generation && !ctl.holding.get() {
                ctl.commit();
            }
        });
        *self.pending.borrow_mut() = Some(pending);
    }

    /// Sends the saving `set`; the state event it causes refreshes the views and ends the local override.
    pub fn commit(self: &Rc<Self>) {
        let (id, patch) = {
            let mut guard = self.pending.borrow_mut();
            let Some(p) = guard.as_mut().filter(|p| p.settle_until.is_none()) else { return };
            p.settle_until = Some(Instant::now() + SETTLE);
            self.generation.set(p.generation + 1);
            (p.id.clone(), p.patch.clone())
        };
        let patch = serde_json::from_value(Value::Object(patch)).unwrap_or_default();
        self.send(Request::Set { id, patch, transient: false });
        let ctl = self.clone();
        glib::timeout_add_local_once(SETTLE, move || ctl.changed());
    }

    /// Adds files to the focused output, top-left with a growing offset, and selects the last one.
    pub fn add_paths(self: &Rc<Self>, paths: Vec<String>) {
        for (i, path) in paths.into_iter().enumerate() {
            let offset = 40 + 24 * i as i32;
            self.add_with(path, self.focus.borrow().clone(), None, offset);
        }
    }

    fn add_with(self: &Rc<Self>, path: String, output: Option<String>, scene: Option<String>, offset: i32) {
        let overlay = OverlayPatch { path: Some(path), output, x: Some(offset), y: Some(offset), ..Default::default() };
        let ctl = self.clone();
        glib::spawn_future_local(async move {
            match call(Request::Add { overlay, scene: scene.clone() }).await {
                Ok(id) => {
                    if scene.as_deref() == ctl.state.borrow().as_ref().map(|s| s.scene.as_str()) {
                        ctl.select(id.as_str().map(String::from));
                    } else if let Some(scene) = scene {
                        ctl.toast(&tr("Added to {}").replace("{}", &scene));
                    }
                }
                Err(e) => ctl.toast(&e.to_string()),
            }
        });
    }

    pub fn add_library(self: &Rc<Self>, path: String, scene: String, output: Option<String>) {
        self.add_with(path, output, Some(scene), 40);
    }

    pub fn add_files(self: &Rc<Self>, files: &gdk::FileList) {
        self.add_paths(files.files().iter().filter_map(|f| Some(f.path()?.to_string_lossy().into_owned())).collect());
    }

    /// Removes an overlay and offers to put the same configuration back.
    pub fn remove(self: &Rc<Self>, overlay: Overlay) {
        self.send(Request::Remove { id: overlay.id.clone() });
        let toast = adw::Toast::builder().title(tr("Overlay removed")).button_label(tr("Undo")).build();
        let ctl = self.clone();
        toast.connect_button_clicked(move |_| {
            if let Ok(patch) = serde_json::to_value(&overlay).and_then(serde_json::from_value) {
                ctl.send(Request::Add { overlay: patch, scene: None });
            }
        });
        self.toasts.add_toast(toast);
    }
}
