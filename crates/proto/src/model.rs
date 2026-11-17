use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    #[default]
    Overlay,
    Top,
    Bottom,
    Background,
}

/// Decoder choice. `auto` enables hardware decoding only for big videos; `hardware` forces it (and its interop
/// memory) for any video; `software` never uses it. Images and alpha containers are always decoded in software.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decode {
    #[default]
    Auto,
    Hardware,
    Software,
}

/// Format of optimized copies. `auto` stores animations with alpha uncompressed (no decoding cost, exact alpha)
/// while they fit `raw_proxy_max_mb` per file and `proxy_cache_mb` in total, and encodes the rest; `compressed`
/// always encodes (VP9 with alpha, H.264 without).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyFormat {
    #[default]
    Auto,
    Compressed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub id: String,
    pub path: String,
    pub output: Option<String>,
    pub anchor: Anchor,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub opacity: f64,
    pub speed: f64,
    pub layer: Layer,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub volume: u32,
    pub visible: bool,
    pub playing: bool,
    /// Frame rate limit; `None` plays at the media rate.
    pub max_fps: Option<u32>,
    pub decode: Decode,
    /// Allows a smaller cached copy of the media when it is drawn much smaller than its size.
    pub optimize: bool,
    /// Lets the overlay go past the monitor edges by up to half its own size, to hide transparent margins; the part
    /// outside is not drawn. Off keeps it inside the monitor (the offset is clamped).
    pub overflow: bool,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            id: String::new(),
            path: String::new(),
            output: None,
            anchor: Anchor::TopLeft,
            x: 0,
            y: 0,
            scale: 1.0,
            width: None,
            height: None,
            opacity: 1.0,
            speed: 1.0,
            layer: Layer::Overlay,
            looping: true,
            volume: 0,
            visible: true,
            playing: true,
            max_fps: None,
            decode: Decode::Auto,
            optimize: true,
            overflow: false,
        }
    }
}

/// Partial overlay used by `add` and `set`. For `width`/`height`/`max_fps`, 0 clears the value;
/// for `output`, an empty string clears it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayPatch {
    pub id: Option<String>,
    pub path: Option<String>,
    pub output: Option<String>,
    pub anchor: Option<Anchor>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub scale: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub opacity: Option<f64>,
    pub speed: Option<f64>,
    pub layer: Option<Layer>,
    #[serde(rename = "loop")]
    pub looping: Option<bool>,
    pub volume: Option<u32>,
    pub visible: Option<bool>,
    pub playing: Option<bool>,
    pub max_fps: Option<u32>,
    pub decode: Option<Decode>,
    pub optimize: Option<bool>,
    pub overflow: Option<bool>,
}

impl OverlayPatch {
    /// Merges every present field into `o`; `id` is never merged.
    pub fn apply(self, o: &mut Overlay) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(v) = self.$f { o.$f = v; })* };
        }
        set!(path, anchor, x, y, scale, opacity, speed, layer, volume, visible, playing, decode, optimize, overflow);
        if let Some(v) = self.max_fps {
            o.max_fps = (v > 0).then_some(v);
        }
        if let Some(v) = self.looping {
            o.looping = v;
        }
        if let Some(v) = self.output {
            o.output = (!v.is_empty()).then_some(v);
        }
        if let Some(v) = self.width {
            o.width = (v > 0).then_some(v);
        }
        if let Some(v) = self.height {
            o.height = (v > 0).then_some(v);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    #[serde(rename = "overlay")]
    pub overlays: Vec<Overlay>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub scene: String,
    pub hwdec: String,
    pub hwdec_min_height: u32,
    pub hide_for_classes: Vec<String>,
    /// Allows cached smaller copies of media that is drawn much smaller (per overlay: `optimize`).
    pub optimize: bool,
    /// Size limit of the cache of those copies.
    pub proxy_cache_mb: u32,
    pub proxy_format: ProxyFormat,
    /// Largest uncompressed copy of one file; bigger ones are encoded instead.
    pub raw_proxy_max_mb: u32,
    /// Hyprland key combination ("SUPER + SHIFT + F1") that loads each scene, bound while the daemon runs.
    pub scene_keys: BTreeMap<String, String>,
    /// Status icon in the panel tray.
    pub tray: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scene: "default".into(),
            hwdec: "auto-safe".into(),
            hwdec_min_height: 720,
            hide_for_classes: Vec::new(),
            optimize: true,
            proxy_cache_mb: 4096,
            proxy_format: ProxyFormat::Auto,
            raw_proxy_max_mb: 1024,
            scene_keys: BTreeMap::new(),
            tray: true,
        }
    }
}

impl Config {
    /// Applies a JSON object of settings over `self`. Unknown keys, wrong types, the scene (use `scene_load`) and
    /// values the daemon cannot work with are rejected, so a bad patch never changes anything.
    pub fn merged(&self, patch: &serde_json::Value) -> Result<Config, String> {
        let (Some(patch), Ok(serde_json::Value::Object(mut fields))) = (patch.as_object(), serde_json::to_value(self))
        else {
            return Err("patch must be a JSON object".into());
        };
        for (key, value) in patch {
            match key.as_str() {
                "scene" => return Err("use scene_load to change the scene".into()),
                k if !fields.contains_key(k) => return Err(format!("unknown setting '{k}'")),
                _ => fields.insert(key.clone(), value.clone()),
            };
        }
        let new: Config = serde_json::from_value(fields.into()).map_err(|e| format!("invalid setting: {e}"))?;
        if new.hwdec.trim().is_empty() || new.hide_for_classes.iter().any(|c| c.trim().is_empty()) {
            return Err("hwdec and hide_for_classes entries must not be empty".into());
        }
        if new.hwdec_min_height == 0 || new.proxy_cache_mb == 0 || new.raw_proxy_max_mb == 0 {
            return Err("hwdec_min_height, proxy_cache_mb and raw_proxy_max_mb must be at least 1".into());
        }
        let mut seen = HashSet::new();
        for (scene, keys) in &new.scene_keys {
            if !crate::paths::valid_scene_name(scene) || !valid_keys(keys) {
                return Err(format!("invalid shortcut '{keys}' for scene '{scene}'"));
            }
            if !seen.insert(normal_keys(keys)) {
                return Err(format!("the shortcut {keys} is used by two scenes"));
            }
        }
        Ok(new)
    }
}

const MODIFIERS: [&str; 4] = ["SUPER", "CTRL", "ALT", "SHIFT"];

/// A key combination as Hyprland writes it: modifiers then one key name, joined by " + ".
pub fn valid_keys(keys: &str) -> bool {
    let parts: Vec<&str> = keys.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last().expect("split yields at least one part");
    !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && mods.iter().all(|m| MODIFIERS.contains(&m.to_ascii_uppercase().as_str()))
}

/// Canonical form of a key combination, to compare two of them: sorted upper-case modifiers, then the key.
pub fn normal_keys(keys: &str) -> String {
    let mut parts: Vec<String> = keys.split('+').map(|p| p.trim().to_ascii_uppercase()).collect();
    let key = parts.pop().unwrap_or_default();
    parts.sort_unstable();
    parts.dedup();
    parts.push(key);
    parts.join(" + ")
}

#[derive(Debug, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    fn new(text: &str, e: &toml::de::Error) -> Self {
        let before = &text[..e.span().map_or(0, |s| s.start).min(text.len())];
        Self {
            line: before.matches('\n').count() + 1,
            column: before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1,
            message: e.message().to_string(),
        }
    }
}

/// Parses a TOML document, returning the value and the dotted paths of unknown keys.
/// Keys of earlier versions that are dropped without a warning; the next save leaves them out. Overlays used to be
/// hidden or paused while a window was fullscreen: the layer alone decides that now.
const RETIRED: [&str; 2] = ["on_fullscreen", "fullscreen_includes_maximized"];

pub fn parse_toml<T: for<'de> Deserialize<'de>>(text: &str) -> Result<(T, Vec<String>), ParseError> {
    let mut unknown = Vec::new();
    let value = toml::Deserializer::parse(text)
        .and_then(|de| {
            serde_ignored::deserialize(de, |p| {
                let p = p.to_string();
                if !RETIRED.iter().any(|r| p.rsplit('.').next() == Some(r)) {
                    unknown.push(p);
                }
            })
        })
        .map_err(|e| ParseError::new(text, &e))?;
    Ok((value, unknown))
}

pub fn to_toml<T: Serialize>(value: &T) -> String {
    toml::to_string_pretty(value).unwrap_or_default()
}

impl Scene {
    pub fn find(&self, id: &str) -> Option<&Overlay> {
        self.overlays.iter().find(|o| o.id == id)
    }

    /// Returns `base`, `base-2`, `base-3`, ... whichever is not used yet.
    pub fn unique_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "overlay" } else { base };
        let taken: HashSet<&str> = self.overlays.iter().map(|o| o.id.as_str()).collect();
        (1..)
            .map(|n| if n == 1 { base.to_string() } else { format!("{base}-{n}") })
            .find(|c| !taken.contains(c.as_str()))
            .unwrap_or_default()
    }

    /// Gives every overlay with an empty or duplicate id a unique one derived from its file stem.
    pub fn normalize(&mut self) {
        let mut seen = HashSet::new();
        for i in 0..self.overlays.len() {
            let id = self.overlays[i].id.clone();
            if id.is_empty() || !seen.insert(id) {
                let stem = file_stem(&self.overlays[i].path);
                self.overlays[i].id = String::new();
                let fresh = self.unique_id(&stem);
                seen.insert(fresh.clone());
                self.overlays[i].id = fresh;
            }
        }
    }
}

pub fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_keys_are_checked() {
        assert!(valid_keys("SUPER + SHIFT + F1") && valid_keys("F13") && valid_keys("ctrl+alt+KP_1"));
        assert!(!valid_keys("") && !valid_keys("SUPER + ") && !valid_keys("HYPER + A") && !valid_keys("A\"; rm"));
        assert_eq!(normal_keys("shift + SUPER + a"), normal_keys("SUPER+SHIFT+A"));
        let config = Config::default();
        assert!(config.merged(&serde_json::json!({ "scene_keys": { "Escena 1": "SUPER + F1" } })).is_ok());
        assert!(config.merged(&serde_json::json!({ "scene_keys": { "a": "SUPER + F1", "b": "super+f1" } })).is_err());
        assert!(config.merged(&serde_json::json!({ "scene_keys": { "../x": "F1" } })).is_err());
    }

    #[test]
    fn overlay_round_trip() {
        let o = Overlay {
            id: "a".into(),
            path: "/x.gif".into(),
            output: Some("eDP-1".into()),
            anchor: Anchor::BottomRight,
            width: Some(300),
            looping: false,
            layer: Layer::Top,
            ..Overlay::default()
        };
        let scene = Scene { overlays: vec![o] };
        let (back, unknown): (Scene, _) = parse_toml(&to_toml(&scene)).unwrap();
        assert_eq!(back, scene);
        assert!(unknown.is_empty());
    }

    #[test]
    fn retired_keys_are_dropped_silently() {
        let text = "[[overlay]]\nid = \"a\"\npath = \"/x.gif\"\non_fullscreen = \"hide\"\nbogus = 1\n";
        let (scene, unknown): (Scene, _) = parse_toml(text).unwrap();
        assert_eq!(scene.overlays.len(), 1);
        assert_eq!(unknown, ["overlay.0.bogus"]);
        let (_, unknown): (Config, _) = parse_toml("fullscreen_includes_maximized = true\n").unwrap();
        assert!(unknown.is_empty());
    }

    #[test]
    fn missing_fields_use_defaults() {
        let (s, _): (Scene, _) = parse_toml("[[overlay]]\npath = \"a.png\"\n").unwrap();
        assert_eq!(s.overlays[0], Overlay { path: "a.png".into(), ..Overlay::default() });
        let (c, _): (Config, _) = parse_toml("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn unknown_keys_are_reported_not_fatal() {
        let (s, unknown): (Scene, _) = parse_toml("[[overlay]]\npath = \"a\"\nmotion = 3\n").unwrap();
        assert_eq!(s.overlays.len(), 1);
        assert_eq!(unknown, vec!["overlay.0.motion"]);
        let (_, unknown): (Config, _) = parse_toml("bogus = true").unwrap();
        assert_eq!(unknown, vec!["bogus"]);
    }

    #[test]
    fn syntax_error_reports_position() {
        let err = parse_toml::<Scene>("[[overlay]]\nx = = 1\n").unwrap_err().to_string();
        assert!(err.starts_with("line 2, column 5: "), "{err}");
    }

    #[test]
    fn patch_merges_and_clears() {
        let mut o = Overlay { width: Some(10), output: Some("A".into()), ..Overlay::default() };
        let p = OverlayPatch {
            x: Some(5),
            width: Some(0),
            output: Some(String::new()),
            looping: Some(false),
            ..Default::default()
        };
        p.apply(&mut o);
        assert_eq!((o.x, o.width, o.output.as_deref(), o.looping), (5, None, None, false));
    }

    #[test]
    fn playback_options_patch_and_parse() {
        let mut o = Overlay::default();
        assert_eq!((o.max_fps, o.decode, o.optimize), (None, Decode::Auto, true));
        OverlayPatch { max_fps: Some(30), decode: Some(Decode::Hardware), optimize: Some(false), ..Default::default() }
            .apply(&mut o);
        assert_eq!((o.max_fps, o.decode, o.optimize), (Some(30), Decode::Hardware, false));
        OverlayPatch { max_fps: Some(0), ..Default::default() }.apply(&mut o);
        assert_eq!(o.max_fps, None);
        let (s, unknown): (Scene, _) =
            parse_toml("[[overlay]]\npath = \"a\"\nmax_fps = 24\ndecode = \"software\"\noptimize = false\n").unwrap();
        assert!(unknown.is_empty());
        assert_eq!(
            (s.overlays[0].max_fps, s.overlays[0].decode, s.overlays[0].optimize),
            (Some(24), Decode::Software, false)
        );
        let (c, _): (Config, _) = parse_toml("proxy_cache_mb = 10").unwrap();
        assert_eq!(
            (c.optimize, c.proxy_cache_mb, c.proxy_format, c.raw_proxy_max_mb),
            (true, 10, ProxyFormat::Auto, 1024)
        );
        let (c, _): (Config, _) = parse_toml("proxy_format = \"compressed\"\nraw_proxy_max_mb = 64").unwrap();
        assert_eq!((c.proxy_format, c.raw_proxy_max_mb), (ProxyFormat::Compressed, 64));
    }

    #[test]
    fn config_patch_merges_and_validates() {
        let base = Config::default();
        let c = base
            .merged(&serde_json::json!({"hwdec": "vaapi", "hide_for_classes": ["mpv"], "proxy_format": "compressed"}));
        let c = c.unwrap();
        assert_eq!(
            (c.hwdec.as_str(), c.hide_for_classes.as_slice(), c.proxy_format),
            ("vaapi", &["mpv".into()][..], ProxyFormat::Compressed)
        );
        assert_eq!(c.proxy_cache_mb, base.proxy_cache_mb);
        for bad in [
            serde_json::json!({"scene": "x"}),
            serde_json::json!({"nope": 1}),
            serde_json::json!({"optimize": "yes"}),
            serde_json::json!({"proxy_format": "zip"}),
            serde_json::json!({"hwdec": " "}),
            serde_json::json!({"proxy_cache_mb": 0}),
            serde_json::json!({"hide_for_classes": [""]}),
            serde_json::json!(3),
        ] {
            assert!(base.merged(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut s = Scene {
            overlays: vec![
                Overlay { id: "duck".into(), ..Default::default() },
                Overlay { id: "duck".into(), path: "~/a/duck.gif".into(), ..Default::default() },
                Overlay { path: "duck.png".into(), ..Default::default() },
            ],
        };
        s.normalize();
        let ids: Vec<_> = s.overlays.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["duck", "duck-2", "duck-3"]);
    }
}
