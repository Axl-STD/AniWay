mod commands;
mod egl;
mod hypr;
mod ipc_server;
mod live;
mod mpv;
mod proxy;
mod stats;
mod store;
mod surface;
mod tray;
mod wayland;

use aniway_proto::geometry::Size;
use aniway_proto::model::{Config, Scene};
use aniway_proto::paths;
use anyhow::{Context, Result};
use calloop::generic::Generic;
use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{channel, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken};
use egl::Egl;
use ipc_server::{bind_socket, read_lines};
use live::{Failure, Live};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::Connection;
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use store::Store;
use wayland::Wl;

const STATS_PERIOD: Duration = Duration::from_secs(5);

/// Delayed actions that coalesce bursts of events.
#[derive(Clone, Copy)]
enum Debounce {
    Reload = 0,
    Screen = 1,
    State = 2,
    Proxy = 3,
}

pub struct App {
    wl: Wl,
    egl: Egl,
    store: Store,
    config: Config,
    scene: Scene,
    live: HashMap<String, Live>,
    failed: HashMap<String, Failure>,
    /// Last known display size per media path, to decide hardware decoding before a file is loaded.
    sizes: HashMap<String, Size>,
    /// Optimized copies per overlay id, with the thread that builds them.
    proxies: HashMap<String, proxy::Job>,
    proxy_gen: u64,
    worker: proxy::Worker,
    next_key: u64,
    mpv_tx: channel::Sender<u64>,
    handle: LoopHandle<'static, App>,
    stop: LoopSignal,
    hidden: bool,
    paused_all: bool,
    snooze: Option<(Instant, RegistrationToken)>,
    hypr: Option<hypr::Hypr>,
    screen: hypr::Screen,
    hypr_backoff: Duration,
    /// Key combinations bound in Hyprland for `scene_keys`.
    bound: Vec<String>,
    /// Tray icon and the (hidden, paused) state its menu shows.
    tray: Option<(tray::Handle, (bool, bool))>,
    /// Deadlines of the pending debounce timers; timers are never removed early, only postponed.
    due: [Option<Instant>; 4],
    /// Connections that receive a state event after every change.
    subscribers: Vec<UnixStream>,
    _watcher: Option<RecommendedWatcher>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Catalogues are read from `<prefix>/share/locale`, or from `target/locale` next to the binary's directory.
fn init_i18n() {
    use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};
    // Only messages: libmpv refuses to work unless numbers keep the C locale.
    setlocale(LocaleCategory::LcMessages, "");
    setlocale(LocaleCategory::LcCType, "");
    let exe = std::env::current_exe().unwrap_or_default();
    if let Some(dir) = ["../share/locale", "../locale"].map(|d| exe.with_file_name(d)).iter().find(|d| d.is_dir()) {
        let _ = bindtextdomain("aniway", dir);
    }
    let _ = bind_textdomain_codeset("aniway", "UTF-8");
    let _ = textdomain("aniway");
}

fn run() -> Result<()> {
    mpv::limit_arenas();
    init_i18n();
    // Our own messages at info; libraries (zbus logs every D-Bus call at info) only when they warn.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,aniwayd=info,aniway_proto=info"))
        .init();
    let exe = std::env::current_exe().ok();
    for note in aniway_proto::legacy::migrate(exe.as_deref()) {
        log::info!("{note}");
    }
    let Some(_lock) = lock_instance()? else {
        eprintln!("aniway is already running");
        std::process::exit(ALREADY_RUNNING);
    };
    let listener = bind_socket()?;

    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init(&conn)?;
    let wl = Wl::bind(conn.clone(), &globals, queue.handle())?;
    let egl = Egl::new(wl.display_ptr())?;
    let mut store = Store::default();
    store.init()?;
    let config = store.load_config(true).unwrap_or_else(fail_to_default).unwrap_or_default();
    let scene = store.load_scene(&config.scene, true).unwrap_or_else(fail_to_default).unwrap_or_default();

    let mut event_loop: EventLoop<'static, App> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    let (mpv_tx, mpv_rx) = channel::channel::<u64>();
    // Signals are blocked in this thread first, so threads spawned later inherit the mask and never take them.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    let (proxy_tx, proxy_rx) = channel::channel::<proxy::Done>();
    let mut app = App {
        wl,
        egl,
        store,
        config,
        scene: Scene::default(),
        live: HashMap::new(),
        failed: HashMap::new(),
        sizes: HashMap::new(),
        proxies: HashMap::new(),
        proxy_gen: 0,
        worker: proxy::Worker::start(proxy_tx),
        next_key: 0,
        mpv_tx,
        handle: handle.clone(),
        stop: event_loop.get_signal(),
        hidden: false,
        paused_all: false,
        snooze: None,
        hypr: hypr::Hypr::detect(),
        screen: hypr::Screen::default(),
        hypr_backoff: Duration::from_millis(500),
        bound: Vec::new(),
        tray: None,
        due: [None; 4],
        subscribers: Vec::new(),
        _watcher: None,
    };

    // The outputs must be fully described (logical size, hence fractional scale) before any player is created, because
    // a cached optimized copy is chosen by the size it is drawn at; the scene is only given to the app afterwards.
    let mut queue = queue;
    for _ in 0..2 {
        queue.roundtrip(&mut app).context("reading the outputs")?;
    }
    app.scene = scene;

    WaylandSource::new(conn, queue).insert(handle.clone()).map_err(|e| e.error)?;
    handle
        .insert_source(mpv_rx, |ev, _, app| {
            if let channel::Event::Msg(key) = ev {
                app.on_mpv(key);
            }
        })
        .map_err(|e| e.error)?;
    handle
        .insert_source(proxy_rx, |ev, _, app| {
            if let channel::Event::Msg(done) = ev {
                app.proxy_done(done);
            }
        })
        .map_err(|e| e.error)?;
    handle.insert_source(signals, |_, _, app| app.stop.stop()).map_err(|e| e.error)?;
    app.start_ipc(listener)?;
    app._watcher = app.start_watcher();
    if app.hypr.is_some() {
        app.connect_hypr();
    } else {
        log::info!("HYPRLAND_INSTANCE_SIGNATURE not set: hide_for_classes and scene shortcuts are disabled");
    }

    app.sync_tray();
    log::info!("aniway daemon ready (scene '{}')", app.config.scene);
    app.reconcile();
    if stats::enabled() {
        let _ = handle.insert_source(Timer::from_duration(STATS_PERIOD), |_, _, app| {
            stats::report(app.live.values().filter(|l| l.surface.is_some()).count(), STATS_PERIOD);
            TimeoutAction::ToDuration(STATS_PERIOD)
        });
    }
    event_loop.run(None, &mut app, |_| {})?;

    log::info!("shutting down");
    app.config.scene_keys.clear();
    app.sync_binds();
    if let Some((tray, _)) = app.tray.take() {
        tray.shutdown().wait();
    }
    // Let the builder notice the cancellation and kill its ffmpeg before the process exits.
    if app.proxies.values().any(|j| j.state == aniway_proto::ipc::ProxyState::Building) {
        app.proxies.clear();
        std::thread::sleep(Duration::from_millis(300));
    }
    for (_, live) in app.live.drain() {
        live.destroy(&app.egl);
    }
    let _ = app.wl.conn.flush();
    let _ = std::fs::remove_file(paths::socket_path());
    Ok(())
}

fn fail_to_default<T>(e: anyhow::Error) -> Option<T> {
    log::error!("{e:#}; starting with defaults");
    None
}

/// Exit status of a second instance; the autostart unit lists it in `RestartPreventExitStatus`.
const ALREADY_RUNNING: i32 = 3;

/// Takes the exclusive single-instance lock (`None` when another daemon holds it); the file must stay open.
fn lock_instance() -> Result<Option<File>> {
    let file = File::create(paths::lock_path()).context("creating lock file")?;
    Ok(rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok().then_some(file))
}

impl App {
    pub(crate) fn start_watcher(&self) -> Option<RecommendedWatcher> {
        let (tx, rx) = channel::channel::<()>();
        let _ = self.handle.insert_source(rx, |ev, _, app| {
            if let channel::Event::Msg(()) = ev {
                app.debounce(Debounce::Reload, Duration::from_millis(200));
            }
        });
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // Reading the files (done by every reload) also raises access events; only changes matter.
            if res.is_ok_and(|event| !event.kind.is_access()) {
                let _ = tx.send(());
            }
        })
        .map_err(|e| log::warn!("file watcher unavailable: {e}"))
        .ok()?;
        for dir in [paths::config_dir(), paths::scenes_dir()] {
            if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
                log::warn!("cannot watch {}: {e}", dir.display());
            }
        }
        Some(watcher)
    }

    pub(crate) fn debounce(&mut self, which: Debounce, delay: Duration) {
        if self.due[which as usize].replace(Instant::now() + delay).is_some() {
            return;
        }
        let _ = self.handle.insert_source(Timer::from_duration(delay), move |_, _, app| {
            let left = app.due[which as usize].and_then(|d| d.checked_duration_since(Instant::now()));
            if let Some(left) = left.filter(|l| !l.is_zero()) {
                return TimeoutAction::ToDuration(left);
            }
            app.due[which as usize] = None;
            match which {
                Debounce::Reload => app.reload(false),
                Debounce::Screen => app.refresh_screen(),
                Debounce::State => app.broadcast_state(),
                Debounce::Proxy => app.proxy_apply(),
            }
            TimeoutAction::Drop
        });
    }

    pub(crate) fn connect_hypr(&mut self) {
        let Some(hypr) = &self.hypr else { return };
        match hypr.events() {
            Ok(stream) => {
                self.hypr_backoff = Duration::from_millis(500);
                let mut buf = Vec::new();
                let source = Generic::new(stream, Interest::READ, Mode::Level);
                let _ = self.handle.insert_source(source, move |_, stream, app| match read_lines(stream, &mut buf) {
                    Ok((lines, false)) => {
                        // A config reload drops the binds made at runtime.
                        if lines.iter().any(|l| l.starts_with("configreloaded>>")) {
                            app.sync_binds();
                        }
                        if lines.iter().any(|l| hypr::is_relevant(l)) {
                            app.debounce(Debounce::Screen, Duration::from_millis(50));
                        }
                        Ok(PostAction::Continue)
                    }
                    _ => {
                        log::warn!("Hyprland event stream lost, reconnecting");
                        app.retry_hypr();
                        Ok(PostAction::Remove)
                    }
                });
                self.sync_binds();
                self.refresh_screen();
            }
            Err(e) => {
                log::warn!("{e:#}");
                self.retry_hypr();
            }
        }
    }

    pub(crate) fn retry_hypr(&mut self) {
        let delay = self.hypr_backoff;
        self.hypr_backoff = (delay * 2).min(Duration::from_secs(8));
        let _ = self.handle.insert_source(Timer::from_duration(delay), |_, _, app| {
            app.connect_hypr();
            TimeoutAction::Drop
        });
    }

    pub(crate) fn refresh_screen(&mut self) {
        let Some(hypr) = &self.hypr else { return };
        match hypr.screen(&self.config) {
            Ok(screen) if screen != self.screen => {
                self.screen = screen;
                self.reconcile();
            }
            Ok(_) => {}
            Err(e) => log::debug!("{e:#}"),
        }
    }
}
