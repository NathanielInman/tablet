//! openvpn3, as plain data. The daemon turns D-Bus values into [`Value`]s
//! and these pure functions turn them into [`Session`]s and say what a
//! session's status means. Every rule here is tested against property dumps
//! recorded from openvpn3-linux v27 with `busctl --system --json=short call
//! net.openvpn.v3.sessions <path> org.freedesktop.DBus.Properties GetAll s
//! net.openvpn.v3.sessions` (addresses replaced).
//!
//! The status codes are openvpn3-linux's `StatusMajor` and `StatusMinor`
//! (its python module, `openvpn3/constants.py`, lists them).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const BUS: &str = "net.openvpn.v3.sessions";
pub const MANAGER_PATH: &str = "/net/openvpn/v3/sessions";
pub const IFACE: &str = "net.openvpn.v3.sessions";

/// A D-Bus value reduced to what openvpn3 and systemd-resolved use.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Str(String),
    Bytes(Vec<u8>),
    /// An array or a struct.
    List(Vec<Value>),
    Dict(Props),
    Other,
}

pub type Props = BTreeMap<String, Value>;

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::UInt(u) => i64::try_from(*u).ok(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt(u) => Some(*u),
            Value::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> &[Value] {
        match self {
            Value::List(v) => v,
            _ => &[],
        }
    }
}

/// A value as `busctl --json=short` prints it: `{"type": "...", "data":
/// ...}`. Used by the tests and fixtures; the daemon reads zbus values.
pub fn from_busctl(v: &serde_json::Value) -> Value {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    match v.get("data") {
        Some(data) => typed(ty, data),
        None => Value::Other,
    }
}

/// `data` read as the D-Bus type `ty`.
pub fn typed(ty: &str, data: &serde_json::Value) -> Value {
    use serde_json::Value as J;
    match (ty.chars().next(), data) {
        (Some('b'), J::Bool(b)) => Value::Bool(*b),
        (Some('s' | 'o' | 'g'), J::String(s)) => Value::Str(s.clone()),
        (Some('y' | 'q' | 'u' | 't'), J::Number(n)) => Value::UInt(n.as_u64().unwrap_or_default()),
        (Some('n' | 'i' | 'x'), J::Number(n)) => Value::Int(n.as_i64().unwrap_or_default()),
        (Some('a'), _) if ty == "ay" => Value::Bytes(
            data.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(|x| x as u8)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        (Some('a'), J::Object(o)) if ty.starts_with("a{") => {
            let inner = &ty[2..ty.len() - 1];
            let vt = &inner[1..];
            Value::Dict(
                o.iter()
                    .map(|(k, v)| {
                        let v = if vt == "v" {
                            from_busctl(v)
                        } else {
                            typed(vt, v)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            )
        }
        (Some('a'), J::Array(a)) => {
            let inner = &ty[1..];
            Value::List(a.iter().map(|x| typed(inner, x)).collect())
        }
        (Some('('), J::Array(a)) => {
            let fields = split_struct(&ty[1..ty.len() - 1]);
            Value::List(
                a.iter()
                    .zip(fields.iter())
                    .map(|(x, t)| typed(t, x))
                    .collect(),
            )
        }
        (Some('v'), _) => from_busctl(data),
        _ => Value::Other,
    }
}

/// The member types of a struct signature body (`ssu` -> `s`, `s`, `u`;
/// `iay` -> `i`, `ay`).
fn split_struct(sig: &str) -> Vec<String> {
    let c: Vec<char> = sig.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < c.len() {
        let start = i;
        i = end_of_type(&c, i);
        out.push(c[start..i].iter().collect());
    }
    out
}

fn end_of_type(c: &[char], i: usize) -> usize {
    match c.get(i) {
        Some('a') => end_of_type(c, i + 1),
        Some(open @ ('(' | '{')) => {
            let close = if *open == '(' { ')' } else { '}' };
            let mut j = i + 1;
            while j < c.len() && c[j] != close {
                j = end_of_type(c, j);
            }
            j + 1
        }
        Some(_) => i + 1,
        None => i,
    }
}

/// `StatusMajor`.
pub mod major {
    pub const UNSET: u32 = 0;
    pub const CFG_ERROR: u32 = 1;
    pub const CONNECTION: u32 = 2;
    pub const SESSION: u32 = 3;
    pub const PKCS11: u32 = 4;
    pub const PROCESS: u32 = 5;
}

/// `StatusMinor`.
pub mod minor {
    pub const UNSET: u32 = 0;
    pub const CFG_ERROR: u32 = 1;
    pub const CFG_OK: u32 = 2;
    pub const CFG_INLINE_MISSING: u32 = 3;
    pub const CFG_REQUIRE_USER: u32 = 4;
    pub const CONN_INIT: u32 = 5;
    pub const CONN_CONNECTING: u32 = 6;
    pub const CONN_CONNECTED: u32 = 7;
    pub const CONN_DISCONNECTING: u32 = 8;
    pub const CONN_DISCONNECTED: u32 = 9;
    pub const CONN_FAILED: u32 = 10;
    pub const CONN_AUTH_FAILED: u32 = 11;
    pub const CONN_RECONNECTING: u32 = 12;
    pub const CONN_PAUSING: u32 = 13;
    pub const CONN_PAUSED: u32 = 14;
    pub const CONN_RESUMING: u32 = 15;
    pub const CONN_DONE: u32 = 16;
    pub const SESS_NEW: u32 = 17;
    pub const SESS_BACKEND_COMPLETED: u32 = 18;
    pub const SESS_REMOVED: u32 = 19;
    pub const SESS_AUTH_USERPASS: u32 = 20;
    pub const SESS_AUTH_CHALLENGE: u32 = 21;
    pub const SESS_AUTH_URL: u32 = 22;
    pub const PKCS11_SIGN: u32 = 23;
    pub const PKCS11_ENCRYPT: u32 = 24;
    pub const PKCS11_DECRYPT: u32 = 25;
    pub const PKCS11_VERIFY: u32 = 26;
    pub const PROC_STARTED: u32 = 27;
    pub const PROC_STOPPED: u32 = 28;
    pub const PROC_KILLED: u32 = 29;
}

/// openvpn3's name for a minor status code, as its logs print it.
pub fn minor_name(m: u32) -> &'static str {
    const NAMES: [&str; 30] = [
        "UNSET",
        "CFG_ERROR",
        "CFG_OK",
        "CFG_INLINE_MISSING",
        "CFG_REQUIRE_USER",
        "CONN_INIT",
        "CONN_CONNECTING",
        "CONN_CONNECTED",
        "CONN_DISCONNECTING",
        "CONN_DISCONNECTED",
        "CONN_FAILED",
        "CONN_AUTH_FAILED",
        "CONN_RECONNECTING",
        "CONN_PAUSING",
        "CONN_PAUSED",
        "CONN_RESUMING",
        "CONN_DONE",
        "SESS_NEW",
        "SESS_BACKEND_COMPLETED",
        "SESS_REMOVED",
        "SESS_AUTH_USERPASS",
        "SESS_AUTH_CHALLENGE",
        "SESS_AUTH_URL",
        "PKCS11_SIGN",
        "PKCS11_ENCRYPT",
        "PKCS11_DECRYPT",
        "PKCS11_VERIFY",
        "PROC_STARTED",
        "PROC_STOPPED",
        "PROC_KILLED",
    ];
    NAMES.get(m as usize).copied().unwrap_or("UNKNOWN")
}

/// A session's `status` property: `(major, minor, message)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Status {
    pub major: u32,
    pub minor: u32,
    pub message: String,
}

/// What a status means for the person at the bar.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// On its way up: a new session, the backend starting, connecting.
    Starting,
    /// Waiting on the person: a web sign-in, a password, a challenge.
    SignIn,
    Connected,
    /// Lost the server and trying again, on its own.
    Reconnecting,
    /// Paused by someone (`openvpn3 session-manage --pause`).
    Paused,
    /// Gave up: the server refused the sign-in, the connection failed, the
    /// config was bad, the backend died. The session object stays behind.
    Failed,
    /// Disconnected or done: the session is going away.
    Ended,
}

impl Status {
    pub fn kind(&self) -> Kind {
        use minor::*;
        match self.minor {
            CONN_CONNECTED => Kind::Connected,
            SESS_AUTH_URL | SESS_AUTH_USERPASS | SESS_AUTH_CHALLENGE | CFG_REQUIRE_USER
            | PKCS11_SIGN | PKCS11_ENCRYPT | PKCS11_DECRYPT | PKCS11_VERIFY => Kind::SignIn,
            CONN_RECONNECTING | CONN_RESUMING => Kind::Reconnecting,
            CONN_PAUSING | CONN_PAUSED => Kind::Paused,
            CONN_FAILED | CONN_AUTH_FAILED | CFG_ERROR | CFG_INLINE_MISSING | PROC_KILLED => {
                Kind::Failed
            }
            CONN_DISCONNECTING | CONN_DISCONNECTED | CONN_DONE | SESS_REMOVED | PROC_STOPPED => {
                Kind::Ended
            }
            // UNSET, CFG_OK, CONN_INIT, CONN_CONNECTING, SESS_NEW,
            // SESS_BACKEND_COMPLETED, PROC_STARTED.
            _ => {
                if self.major == major::CFG_ERROR {
                    Kind::Failed
                } else {
                    Kind::Starting
                }
            }
        }
    }

    /// The sign-in page openvpn3 announced with `SESS_AUTH_URL`.
    pub fn url(&self) -> Option<String> {
        if self.minor != minor::SESS_AUTH_URL {
            return None;
        }
        find_url(&self.message)
    }

    /// One line for the drawer: what openvpn3 said, in words.
    pub fn describe(&self) -> String {
        use minor::*;
        let said = self.message.trim();
        let base = match self.minor {
            CONN_CONNECTED => "connected",
            CONN_AUTH_FAILED => "the server refused the sign-in",
            CONN_FAILED => "the connection failed",
            CFG_ERROR | CFG_INLINE_MISSING => "openvpn3 could not use the config",
            PROC_KILLED => "the openvpn3 backend was killed",
            CONN_RECONNECTING => "lost the server, reconnecting",
            CONN_RESUMING => "resuming",
            CONN_PAUSING | CONN_PAUSED => "paused",
            SESS_AUTH_URL => "waiting for the web sign-in",
            SESS_AUTH_USERPASS => "waiting for a username and password",
            SESS_AUTH_CHALLENGE => "waiting for a challenge answer",
            CFG_REQUIRE_USER => "the config needs credentials",
            CONN_DISCONNECTING => "disconnecting",
            CONN_DISCONNECTED | CONN_DONE | SESS_REMOVED | PROC_STOPPED => "disconnected",
            CONN_CONNECTING | CONN_INIT => "connecting",
            _ => "starting",
        };
        // A URL is the sign-in page, not something to show in a sentence.
        if said.is_empty() || self.minor == SESS_AUTH_URL || said.starts_with("http") {
            base.to_string()
        } else {
            format!("{base}: {said}")
        }
    }
}

/// The first http(s) URL in a message.
pub fn find_url(s: &str) -> Option<String> {
    let i = s.find("https://").or_else(|| s.find("http://"))?;
    let url: String = s[i..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>'))
        .collect();
    (url.len() > 8).then_some(url)
}

/// One openvpn3 session, from its properties.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Session {
    /// `/net/openvpn/v3/sessions/<id>`.
    pub path: String,
    /// The config it was started from: the `.ovpn` path given to
    /// `session-start --config`, or an imported config's name.
    pub config_name: String,
    /// The server, from the config's `remote`.
    pub server: String,
    /// The tunnel interface (`tun0`), once there is one.
    pub device: String,
    /// Unix seconds.
    pub created: f64,
    pub status: Status,
    /// `(protocol, address, port)` of the server it is talking to.
    pub transport: Option<(String, String, u32)>,
    /// openvpn3's counters: `BYTES_IN`, `BYTES_OUT`, `TUN_BYTES_IN`,
    /// `KEEPALIVE_TIMEOUT`, `N_RECONNECT`, `AUTH_FAILED`, ...
    pub stats: BTreeMap<String, i64>,
    pub backend_pid: u32,
    pub owner: u32,
}

impl Session {
    pub fn from_props(path: &str, p: &Props) -> Session {
        let s = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let status = p
            .get("status")
            .map(Value::as_list)
            .filter(|l| l.len() == 3)
            .map(|l| Status {
                major: l[0].as_u64().unwrap_or_default() as u32,
                minor: l[1].as_u64().unwrap_or_default() as u32,
                message: l[2].as_str().unwrap_or_default().to_string(),
            })
            .unwrap_or_default();
        let transport = p
            .get("connected_to")
            .map(Value::as_list)
            .filter(|l| l.len() == 3)
            .and_then(|l| {
                let host = l[1].as_str()?.to_string();
                (!host.is_empty()).then(|| {
                    (
                        l[0].as_str().unwrap_or_default().to_string(),
                        host,
                        l[2].as_u64().unwrap_or_default() as u32,
                    )
                })
            });
        Session {
            path: path.to_string(),
            config_name: s("config_name"),
            server: s("session_name"),
            device: s("device_name"),
            created: p
                .get("session_created")
                .and_then(Value::as_u64)
                .unwrap_or_default() as f64,
            status,
            transport,
            stats: stats(p.get("statistics")),
            backend_pid: p
                .get("backend_pid")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
            owner: p.get("owner").and_then(Value::as_u64).unwrap_or_default() as u32,
        }
    }

    pub fn stat(&self, name: &str) -> i64 {
        self.stats.get(name).copied().unwrap_or_default()
    }

    /// Why a session that is not connected failed, in words: the status,
    /// or the counters when the status is vague (a session whose sign-in
    /// was refused can sit at `CONN_CONNECTING` with `AUTH_FAILED` = 1).
    pub fn failure(&self) -> Option<String> {
        if self.status.kind() == Kind::Failed {
            return Some(self.status.describe());
        }
        if self.status.kind() != Kind::Connected && self.stat("AUTH_FAILED") > 0 {
            return Some("the server refused the sign-in (AUTH_FAILED)".into());
        }
        None
    }
}

/// `statistics`, `a{sx}`.
pub fn stats(v: Option<&Value>) -> BTreeMap<String, i64> {
    match v {
        Some(Value::Dict(d)) => d
            .iter()
            .filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Props {
        let text = match name {
            "connected" => include_str!("../fixtures/session-connected.json"),
            "auth-failed" => include_str!("../fixtures/session-auth-failed.json"),
            _ => unreachable!(),
        };
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        match from_busctl(&serde_json::json!({"type": v["type"], "data": v["data"][0]})) {
            Value::Dict(d) => d,
            other => panic!("not a dict: {other:?}"),
        }
    }

    #[test]
    fn reads_a_recorded_session() {
        let s = Session::from_props("/net/openvpn/v3/sessions/abc", &fixture("connected"));
        assert_eq!(s.config_name, "/home/user/work.ovpn");
        assert_eq!(s.server, "vpn.example.com");
        assert_eq!(s.device, "tun0");
        assert_eq!(s.created, 1_794_503_231.0);
        assert_eq!(s.status.kind(), Kind::Connected);
        assert_eq!(
            s.transport,
            Some(("udp".into(), "198.51.100.7".into(), 1194))
        );
        assert_eq!(s.stat("BYTES_IN"), 1_522_947);
        assert_eq!(s.stat("N_RECONNECT"), 1);
        assert_eq!(s.owner, 1000);
        assert_eq!(s.failure(), None);
    }

    #[test]
    fn a_refused_sign_in_is_a_failure() {
        let s = Session::from_props("/x", &fixture("auth-failed"));
        assert_eq!(s.status.kind(), Kind::Starting);
        assert_eq!(
            s.failure().as_deref(),
            Some("the server refused the sign-in (AUTH_FAILED)")
        );
        assert!(s.transport.is_none());
    }

    #[test]
    fn kinds() {
        let k = |major, minor| {
            Status {
                major,
                minor,
                message: String::new(),
            }
            .kind()
        };
        assert_eq!(k(2, minor::CONN_CONNECTED), Kind::Connected);
        assert_eq!(k(3, minor::SESS_AUTH_URL), Kind::SignIn);
        assert_eq!(k(2, minor::CONN_RECONNECTING), Kind::Reconnecting);
        assert_eq!(k(2, minor::CONN_PAUSED), Kind::Paused);
        assert_eq!(k(2, minor::CONN_AUTH_FAILED), Kind::Failed);
        assert_eq!(k(5, minor::PROC_KILLED), Kind::Failed);
        assert_eq!(k(2, minor::CONN_DISCONNECTED), Kind::Ended);
        assert_eq!(k(0, minor::UNSET), Kind::Starting);
        assert_eq!(k(3, minor::SESS_BACKEND_COMPLETED), Kind::Starting);
        assert_eq!(k(1, 99), Kind::Failed);
        assert_eq!(minor_name(22), "SESS_AUTH_URL");
        assert_eq!(minor_name(99), "UNKNOWN");
    }

    #[test]
    fn sign_in_urls() {
        let s = Status {
            major: 3,
            minor: minor::SESS_AUTH_URL,
            message: "https://login.example.com/auth?state=abc".into(),
        };
        assert_eq!(
            s.url().as_deref(),
            Some("https://login.example.com/auth?state=abc")
        );
        assert_eq!(s.describe(), "waiting for the web sign-in");
        assert_eq!(
            find_url("Open this URL: https://x.example.com/a b").as_deref(),
            Some("https://x.example.com/a")
        );
        assert_eq!(find_url("nothing here"), None);
        let failed = Status {
            major: 2,
            minor: minor::CONN_FAILED,
            message: "TLS handshake timeout".into(),
        };
        assert_eq!(
            failed.describe(),
            "the connection failed: TLS handshake timeout"
        );
        assert_eq!(failed.url(), None);
    }

    #[test]
    fn struct_signatures() {
        assert_eq!(split_struct("ssu"), vec!["s", "s", "u"]);
        assert_eq!(split_struct("iay"), vec!["i", "ay"]);
        assert_eq!(split_struct("ia(iay)s"), vec!["i", "a(iay)", "s"]);
        let v = typed(
            "a(iay)",
            &serde_json::json!([
                [2, [10, 0, 0, 2]],
                [10, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]]
            ]),
        );
        assert_eq!(v.as_list().len(), 2);
        assert_eq!(v.as_list()[0].as_list()[1], Value::Bytes(vec![10, 0, 0, 2]));
    }
}
