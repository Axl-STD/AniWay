use crate::geometry::{Rect, Size};
use crate::model::{Config, Overlay, OverlayPatch};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    State,
    Add {
        overlay: OverlayPatch,
        #[serde(default)]
        scene: Option<String>,
    },
    /// `transient` applies the change live without saving the scene.
    Set {
        id: String,
        patch: OverlayPatch,
        #[serde(default)]
        transient: bool,
    },
    Remove {
        id: String,
    },
    Show {
        id: Option<String>,
    },
    Hide {
        id: Option<String>,
        for_secs: Option<u64>,
    },
    Toggle {
        id: Option<String>,
    },
    Pause {
        id: Option<String>,
    },
    Resume {
        id: Option<String>,
    },
    TogglePause {
        id: Option<String>,
    },
    SceneList,
    SceneLoad {
        name: String,
    },
    SceneSave {
        name: String,
    },
    SceneDelete {
        name: String,
    },
    /// Creates an empty scene and makes it the active one; the previous scene stays as it was.
    SceneNew {
        name: String,
    },
    /// Renames a scene file; the active scene and its shortcut follow.
    SceneRename {
        name: String,
        to: String,
    },
    Reload,
    /// Merges a JSON object of `config.toml` settings, validates, saves and applies them.
    ConfigSet {
        patch: Value,
    },
    /// After the response, the connection receives `{"event":"state","data":<StateData>}` lines.
    Subscribe,
    Quit,
}

#[derive(Serialize)]
struct Envelope<'a> {
    v: u32,
    #[serde(flatten)]
    request: &'a Request,
}

pub fn encode_request(request: &Request) -> String {
    serde_json::to_string(&Envelope { v: VERSION, request }).unwrap_or_default()
}

/// Parses one request line, rejecting a missing or unsupported protocol version.
pub fn parse_request(line: &str) -> Result<Request, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    match value.get("v").and_then(Value::as_u64) {
        Some(v) if v == u64::from(VERSION) => {}
        other => return Err(format!("unsupported protocol version {other:?}, expected {VERSION}")),
    }
    serde_json::from_value(value).map_err(|e| format!("invalid request: {e}"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Self { ok: true, data: serde_json::to_value(data).ok(), error: None }
    }

    pub fn err(message: impl ToString) -> Self {
        Self { ok: false, data: None, error: Some(message.to_string()) }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub description: String,
    pub rect: Rect,
    pub scale: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayInfo {
    #[serde(flatten)]
    pub config: Overlay,
    pub rect: Option<Rect>,
    pub media: Option<Size>,
    pub hwdec: Option<String>,
    pub error: Option<String>,
    pub shown: bool,
    pub paused: bool,
    /// Frames dropped or delivered late since the player was created.
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub proxy: ProxyState,
    /// Why the optimized copy could not be built; the original keeps playing, so `error` stays empty.
    #[serde(default)]
    pub proxy_error: Option<String>,
}

/// State of the optimized copy of an overlay's media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyState {
    #[default]
    Off,
    Building,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateData {
    pub scene: String,
    pub outputs: Vec<OutputInfo>,
    pub overlays: Vec<OverlayInfo>,
    pub hidden: bool,
    pub paused: bool,
    pub snooze_secs_left: Option<u64>,
    pub config: Config,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_is_persistent_unless_transient() {
        let set = |extra: &str| parse_request(&format!(r#"{{"v":1,"cmd":"set","id":"a","patch":{{}}{extra}}}"#));
        assert!(matches!(set(""), Ok(Request::Set { transient: false, .. })));
        assert!(matches!(set(r#","transient":true"#), Ok(Request::Set { transient: true, .. })));
        assert_eq!(parse_request(r#"{"v":1,"cmd":"subscribe"}"#), Ok(Request::Subscribe));
    }

    #[test]
    fn request_round_trip() {
        let reqs = [
            Request::State,
            Request::Add {
                overlay: OverlayPatch { path: Some("a.gif".into()), x: Some(3), ..Default::default() },
                scene: None,
            },
            Request::Set {
                id: "a".into(),
                patch: OverlayPatch { opacity: Some(0.5), ..Default::default() },
                transient: false,
            },
            Request::Hide { id: None, for_secs: Some(30) },
            Request::SceneSave { name: "work".into() },
            Request::ConfigSet { patch: serde_json::json!({"optimize": false}) },
            Request::Quit,
        ];
        for r in reqs {
            assert_eq!(parse_request(&encode_request(&r)).unwrap(), r);
        }
    }

    #[test]
    fn optional_fields_may_be_missing_and_extra_keys_ignored() {
        let r = parse_request(r#"{"v":1,"cmd":"hide","extra":1}"#).unwrap();
        assert_eq!(r, Request::Hide { id: None, for_secs: None });
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(parse_request("{").unwrap_err().contains("invalid JSON"));
        assert!(parse_request(r#"{"cmd":"state"}"#).unwrap_err().contains("version"));
        assert!(parse_request(r#"{"v":2,"cmd":"state"}"#).unwrap_err().contains("version"));
        assert!(parse_request(r#"{"v":1,"cmd":"nope"}"#).unwrap_err().contains("invalid request"));
        assert!(parse_request(r#"{"v":1,"cmd":"remove"}"#).unwrap_err().contains("id"));
    }

    #[test]
    fn response_shapes() {
        assert_eq!(Response::ok(1).to_line(), r#"{"ok":true,"data":1}"#);
        assert_eq!(Response::err("boom").to_line(), r#"{"ok":false,"error":"boom"}"#);
    }

    #[test]
    fn overlay_info_flattens_config() {
        let info = OverlayInfo {
            config: Overlay { id: "a".into(), ..Default::default() },
            rect: Some(Rect { x: 1, y: 2, w: 3, h: 4 }),
            media: None,
            hwdec: Some("nvdec".into()),
            error: None,
            shown: true,
            paused: false,
            dropped: 0,
            proxy: ProxyState::Off,
            proxy_error: None,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["id"], "a");
        assert_eq!(v["hwdec"], "nvdec");
        assert_eq!(serde_json::from_value::<OverlayInfo>(v).unwrap(), info);
    }
}
