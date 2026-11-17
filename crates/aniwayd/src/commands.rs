use crate::live::output_size;
use crate::{hypr, App};
use aniway_proto::geometry::place;
use aniway_proto::ipc::{OutputInfo, OverlayInfo, ProxyState, Request, Response, StateData};
use aniway_proto::model::{file_stem, Config, Overlay, Scene};
use aniway_proto::paths;
use anyhow::{anyhow, bail, Result};
use calloop::timer::{TimeoutAction, Timer};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn respond(&mut self, request: Result<Request, String>) -> Response {
        match request {
            Err(e) => Response::err(e),
            Ok(request) => match self.exec(request) {
                Ok(data) => Response::ok(data),
                Err(e) => Response::err(format!("{e:#}")),
            },
        }
    }

    pub(crate) fn exec(&mut self, request: Request) -> Result<Value> {
        match request {
            Request::State => return Ok(serde_json::to_value(self.state())?),
            Request::Quit => {
                self.stop.stop();
                self.stop.wakeup();
            }
            Request::Reload => self.reload(true),
            Request::SceneList => {
                return Ok(json!({ "active": self.config.scene, "scenes": self.store.scene_names() }))
            }
            Request::SceneLoad { name } => self.scene_load(&name)?,
            Request::SceneSave { name } => {
                valid_scene(&name)?;
                if paths::scene_file(&name).exists() {
                    bail!("scene '{name}' already exists");
                }
                self.store.save_scene(&name, &self.scene)?;
            }
            Request::SceneDelete { name } => {
                valid_scene(&name)?;
                if name == self.config.scene {
                    bail!("cannot delete the active scene");
                }
                self.store.delete_scene(&name)?;
                if self.config.scene_keys.contains_key(&name) {
                    let mut config = self.config.clone();
                    config.scene_keys.remove(&name);
                    self.store.save_config(&config)?;
                    self.apply_config(config);
                }
            }
            Request::SceneRename { name, to } => self.scene_rename(&name, &to)?,
            Request::SceneNew { name } => {
                valid_scene(&name)?;
                if paths::scene_file(&name).exists() {
                    bail!("scene '{name}' already exists");
                }
                self.store.save_scene(&name, &Default::default())?;
                self.scene_load(&name)?;
            }
            Request::ConfigSet { patch } => {
                let config = self.config.merged(&patch).map_err(|e| anyhow!(e))?;
                self.check_keys(&config)?;
                self.store.save_config(&config)?;
                self.apply_config(config);
            }
            Request::Show { id: None } => self.set_hidden(false, None),
            Request::Hide { id: None, for_secs } => self.set_hidden(true, for_secs),
            Request::Toggle { id: None } => {
                let hide = !(self.hidden || self.snooze.is_some());
                self.set_hidden(hide, None);
            }
            Request::Pause { id: None } => self.paused_all = true,
            Request::Resume { id: None } => self.paused_all = false,
            Request::TogglePause { id: None } => self.paused_all = !self.paused_all,
            Request::Hide { for_secs: Some(_), .. } => bail!("--for applies to the global hide, not to one overlay"),
            Request::Add { overlay } => {
                let path = overlay.path.clone().ok_or_else(|| anyhow!("path is required"))?;
                check_path(&path)?;
                let mut ov = Overlay::default();
                let id = overlay.id.clone();
                overlay.apply(&mut ov);
                if ov.output.is_none() {
                    ov.output = self.default_output();
                }
                ov.id = match id {
                    Some(id) if self.scene.find(&id).is_some() => {
                        bail!("overlay '{id}' already exists")
                    }
                    Some(id) if !id.is_empty() => id,
                    _ => self.scene.unique_id(&file_stem(&ov.path)),
                };
                let id = ov.id.clone();
                self.mutate(|scene| scene.overlays.push(ov))?;
                self.reconcile();
                return Ok(json!(id));
            }
            Request::Remove { id } => {
                self.overlay(&id)?;
                self.mutate(|scene| scene.overlays.retain(|o| o.id != id))?;
            }
            Request::Set { id, patch, transient } => {
                self.overlay(&id)?;
                if let Some(path) = &patch.path {
                    check_path(path)?;
                }
                if transient {
                    if let Some(o) = self.scene.overlays.iter_mut().find(|o| o.id == id) {
                        patch.apply(o);
                    }
                } else {
                    self.edit(&id, |o| patch.apply(o))?;
                }
                self.failed.remove(&id);
            }
            Request::Subscribe => {}
            Request::Show { id: Some(id) } => self.edit(&id, |o| o.visible = true)?,
            Request::Hide { id: Some(id), .. } => self.edit(&id, |o| o.visible = false)?,
            Request::Toggle { id: Some(id) } => self.edit(&id, |o| o.visible = !o.visible)?,
            Request::Pause { id: Some(id) } => self.edit(&id, |o| o.playing = false)?,
            Request::Resume { id: Some(id) } => self.edit(&id, |o| o.playing = true)?,
            Request::TogglePause { id: Some(id) } => self.edit(&id, |o| o.playing = !o.playing)?,
        }
        self.reconcile();
        Ok(Value::Null)
    }

    pub(crate) fn overlay(&self, id: &str) -> Result<&Overlay> {
        self.scene.find(id).ok_or_else(|| anyhow!("no overlay with id '{id}'"))
    }

    pub(crate) fn edit(&mut self, id: &str, f: impl FnOnce(&mut Overlay)) -> Result<()> {
        self.overlay(id)?;
        self.mutate(|scene| {
            if let Some(o) = scene.overlays.iter_mut().find(|o| o.id == id) {
                f(o);
            }
        })
    }

    /// Applies a scene change and persists it; the in-memory scene is restored if saving fails.
    pub(crate) fn mutate(&mut self, f: impl FnOnce(&mut Scene)) -> Result<()> {
        let before = self.scene.clone();
        f(&mut self.scene);
        if let Err(e) = self.store.save_scene(&self.config.scene, &self.scene) {
            self.scene = before;
            return Err(e);
        }
        Ok(())
    }

    pub(crate) fn scene_load(&mut self, name: &str) -> Result<()> {
        valid_scene(name)?;
        if !paths::scene_file(name).exists() {
            bail!("scene '{name}' does not exist");
        }
        let scene = self.store.load_scene(name, true)?.unwrap_or_default();
        let config = Config { scene: name.into(), ..self.config.clone() };
        self.store.save_config(&config)?;
        (self.config, self.scene) = (config, scene);
        Ok(())
    }

    pub(crate) fn set_hidden(&mut self, hidden: bool, for_secs: Option<u64>) {
        if let Some((_, token)) = self.snooze.take() {
            self.handle.remove(token);
        }
        self.hidden = hidden && for_secs.is_none();
        let Some(secs) = for_secs.filter(|_| hidden) else {
            return;
        };
        let delay = Duration::from_secs(secs);
        let timer = Timer::from_duration(delay);
        let token = self.handle.insert_source(timer, |_, _, app| {
            app.snooze = None;
            app.reconcile();
            TimeoutAction::Drop
        });
        self.snooze = token.ok().map(|t| (Instant::now() + delay, t));
    }

    pub(crate) fn apply_config(&mut self, config: Config) {
        if config.hwdec != self.config.hwdec {
            self.live.values_mut().filter(|l| l.player.hwdec_on).for_each(|l| l.player.set_hwdec(&config.hwdec));
        }
        let (keys, tray) = (config.scene_keys != self.config.scene_keys, config.tray != self.config.tray);
        self.config = config;
        if keys {
            self.sync_binds();
        }
        if tray {
            self.sync_tray();
        }
        self.refresh_screen();
    }

    fn scene_rename(&mut self, name: &str, to: &str) -> Result<()> {
        valid_scene(name)?;
        valid_scene(to)?;
        if !paths::scene_file(name).exists() {
            bail!("scene '{name}' does not exist");
        }
        if name == to {
            return Ok(());
        }
        // A case-only rename is the same file on case-insensitive file systems.
        if paths::scene_file(to).exists() && !name.eq_ignore_ascii_case(to) {
            bail!("scene '{to}' already exists");
        }
        self.store.rename_scene(name, to)?;
        let mut config = self.config.clone();
        if config.scene == name {
            config.scene = to.into();
        }
        if let Some(keys) = config.scene_keys.remove(name) {
            config.scene_keys.insert(to.into(), keys);
        }
        self.store.save_config(&config)?;
        self.apply_config(config);
        Ok(())
    }

    /// Rejects scene shortcuts that Hyprland already uses for something else.
    fn check_keys(&self, config: &Config) -> Result<()> {
        let Some(hypr) = &self.hypr else { return Ok(()) };
        let ours: Vec<_> = self.bound.iter().map(|k| hypr::combo(k)).collect();
        let taken = hypr.binds().unwrap_or_default();
        match config.scene_keys.values().find(|k| !ours.contains(&hypr::combo(k)) && taken.contains(&hypr::combo(k))) {
            Some(keys) => bail!("{keys} is already a Hyprland shortcut"),
            None => Ok(()),
        }
    }

    /// Makes our Hyprland binds match `scene_keys`: each runs `aniway scene load <name>`.
    pub(crate) fn sync_binds(&mut self) {
        let Some(hypr) = &self.hypr else { return };
        for keys in self.bound.drain(..) {
            if let Err(e) = hypr.bind(&keys, None) {
                log::debug!("unbinding {keys}: {e:#}");
            }
        }
        let cli = std::env::current_exe()
            .map(|e| e.with_file_name("aniway"))
            .ok()
            .filter(|c| c.exists())
            .map_or("aniway".into(), |c| c.to_string_lossy().into_owned());
        for (scene, keys) in &self.config.scene_keys {
            match hypr.bind(keys, Some(&format!("{} scene load {}", quote(&cli), quote(scene)))) {
                Ok(()) => self.bound.push(keys.clone()),
                Err(e) => log::warn!("cannot bind {keys} to scene '{scene}': {e:#}"),
            }
        }
    }

    /// Re-reads config and active scene when their content changed (or always when `force`).
    pub(crate) fn reload(&mut self, force: bool) {
        if force {
            self.failed.clear();
        }
        match self.store.load_config(force) {
            Ok(Some(config)) => self.apply_config(config),
            Ok(None) => {}
            Err(e) => log::error!("{e:#}; keeping previous settings"),
        }
        match self.store.load_scene(&self.config.scene, force) {
            Ok(Some(scene)) => {
                log::info!("scene '{}' reloaded ({} overlays)", self.config.scene, scene.overlays.len());
                self.scene = scene;
            }
            Ok(None) => {}
            Err(e) => log::error!("{e:#}; keeping previous scene"),
        }
        self.reconcile();
    }

    pub(crate) fn state(&self) -> StateData {
        let outputs = self.wl.output_infos();
        let overlays = self.scene.overlays.iter().map(|ov| {
            let (live, job) = (self.live.get(&ov.id), self.proxies.get(&ov.id));
            let output = self.wl.find_output(ov.output.as_deref());
            let media = live.and_then(|l| l.media).or_else(|| self.sizes.get(&ov.path).copied());
            OverlayInfo {
                config: ov.clone(),
                rect: output.zip(media).map(|((_, info), media)| place(output_size(&info), media, ov)),
                media,
                hwdec: live.and_then(|l| l.player.hwdec_current()),
                error: live.and_then(|l| l.error.clone()).or_else(|| self.failed.get(&ov.id).map(|f| f.error.clone())),
                shown: live.is_some_and(|l| l.surface.is_some()),
                paused: live.is_none_or(|l| l.paused),
                dropped: live.map_or(0, |l| l.player.dropped()),
                proxy: job.map_or(ProxyState::Off, |j| j.state),
                proxy_error: job.and_then(|j| j.error.clone()),
            }
        });
        StateData {
            scene: self.config.scene.clone(),
            outputs: outputs
                .iter()
                .map(|(_, i)| {
                    let size = output_size(i);
                    let (pw, ph) =
                        i.modes.iter().find(|m| m.current).map_or((size.w as i32, size.h as i32), |m| m.dimensions);
                    let (x, y) = i.logical_position.unwrap_or(i.location);
                    OutputInfo {
                        name: i.name.clone().unwrap_or_default(),
                        description: i.description.clone().unwrap_or_default(),
                        rect: aniway_proto::geometry::Rect { x, y, w: size.w, h: size.h },
                        scale: (f64::from(pw.max(ph)) / f64::from(size.w.max(size.h)) * 100.0).round() / 100.0,
                    }
                })
                .collect(),
            overlays: overlays.collect(),
            hidden: self.hidden,
            paused: self.paused_all,
            snooze_secs_left: self
                .snooze
                .as_ref()
                .map(|(at, _)| at.saturating_duration_since(Instant::now()).as_secs()),
            config: self.config.clone(),
        }
    }
}

pub(crate) fn valid_scene(name: &str) -> Result<()> {
    if !paths::valid_scene_name(name) {
        bail!(
            "invalid scene name '{name}' (no '/', no leading '.' or spaces, at most {} characters)",
            paths::SCENE_NAME_MAX
        );
    }
    Ok(())
}

/// Single-quoted for `sh -c`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

pub(crate) fn check_path(path: &str) -> Result<()> {
    if !paths::is_url(path) && !paths::expand_tilde(path).exists() {
        bail!("file not found: {path}");
    }
    Ok(())
}
