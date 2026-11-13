//! The daemon socket: newline-delimited JSON, one request per line. The
//! shapes follow Roundhouse's, Switchyard's and Tannoy's (`{"cmd":"toggle",
//! "anchor":{...}}`, `{"event":"show",...}`), so a sibling can talk to any
//! of them without a shared crate.

use crate::Anchor;
use crate::model::Snapshot;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SubKind {
    /// Snapshots and show/hide events, for the drawer.
    Board,
    /// Waybar JSON lines, already formatted.
    Bar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Snapshot,
    Subscribe {
        kind: SubKind,
    },
    Toggle {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Show {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Hide,
    /// The drawer reports whether it is on screen.
    BoardVisible {
        visible: bool,
    },
    /// Starts a profile's session (`null`: the default profile).
    Connect {
        #[serde(default)]
        profile: Option<String>,
    },
    /// Ends a profile's sessions (`null`: every session Tablet knows).
    Disconnect {
        #[serde(default)]
        profile: Option<String>,
    },
    /// Ends the profile's session and starts a new one.
    Reconnect {
        #[serde(default)]
        profile: Option<String>,
    },
    /// The lamp's click: connect when off, disconnect when held, reconnect
    /// when stale, failed or dropped, open the sign-in page while one is
    /// waiting, accept a waiting request.
    Flip,
    /// Checks a profile now (`null`: every connected profile) and replies
    /// once the checks are done, with the snapshot.
    Check {
        #[serde(default)]
        profile: Option<String>,
    },
    /// Dismisses a dropped or failed profile; a dead session is
    /// disconnected.
    Ack {
        #[serde(default)]
        profile: Option<String>,
    },
    /// Opens the sign-in page openvpn3 is waiting on, again.
    OpenSignIn {
        #[serde(default)]
        profile: Option<String>,
    },
    /// An agent asks for a profile. The reply comes once the profile is
    /// held (`ok`), or with an `error` when you decline, the connect fails
    /// or `timeout` seconds pass.
    Ask {
        profile: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        command: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        requester: String,
        #[serde(default = "default_ask_timeout")]
        timeout: u64,
    },
    /// Your answer to a request: Connect, or Not now.
    Answer {
        id: u64,
        connect: bool,
    },
    /// Runs the doctor now and replies with the snapshot.
    Doctor,
    /// Runs `doctor.repair`.
    Repair,
    /// Stops the daemon and the drawer, for `tablet restart` and for
    /// replacing a daemon whose binary was upgraded.
    Quit,
}

fn default_ask_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Box<Snapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What a command did, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// In a `ping` reply: the daemon's binary was replaced by an upgrade, so
    /// it is running old code and should be restarted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply {
            ok: true,
            ..Default::default()
        }
    }

    pub fn said(message: impl ToString) -> Reply {
        Reply {
            ok: true,
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    pub fn err(e: impl ToString) -> Reply {
        Reply {
            ok: false,
            error: Some(e.to_string()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        snapshot: Box<Snapshot>,
    },
    Show {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Toggle {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Hide,
    /// The daemon is going away for good (a restart or an upgrade): the
    /// drawer exits, and the daemon starts the new one when it is needed.
    Quit,
}

/// The line a waybar `custom` module with `"return-type": "json"` reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BarLine {
    pub text: String,
    /// The lamp's state (`held`, `stale`, ...), then `pulse` while it waits
    /// on something.
    pub class: Vec<String>,
    pub alt: String,
    pub tooltip: String,
}

/// Sends one request and reads one reply, waiting at most `timeout`.
pub fn request_within(req: &Request, timeout: Duration) -> Result<Reply> {
    let path = crate::paths::socket_path();
    exchange(crate::transport::connect_to(&path, Some(timeout))?, req)
}

/// Sends one request and reads one reply, with the friendly error when the
/// daemon is not running.
pub fn request(req: &Request) -> Result<Reply> {
    let s = crate::transport::connect()?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    exchange(s, req)
}

/// Sends one request and waits for its reply as long as it takes (an
/// agent's `ask`).
pub fn request_patient(req: &Request) -> Result<Reply> {
    let s = crate::transport::connect()?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    exchange(s, req)
}

fn exchange(mut s: crate::transport::Stream, req: &Request) -> Result<Reply> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(s);
    let mut out = String::new();
    reader.read_line(&mut out)?;
    serde_json::from_str(&out).map_err(|e| anyhow!("bad reply from daemon: {e}"))
}

/// Subscribes and yields raw lines until the daemon goes away.
pub fn subscribe_raw(kind: SubKind) -> Result<impl Iterator<Item = std::io::Result<String>>> {
    let mut s = crate::transport::connect()?;
    let mut line = serde_json::to_string(&Request::Subscribe { kind })?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    Ok(BufReader::new(s).lines())
}

/// Subscribes and yields parsed events.
pub fn subscribe(kind: SubKind) -> Result<impl Iterator<Item = Result<Event>>> {
    Ok(subscribe_raw(kind)?.map(|l| {
        let l = l?;
        serde_json::from_str::<Event>(&l).map_err(|e| anyhow!("bad event: {e}"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shapes() {
        let r: Request = serde_json::from_str(r#"{"cmd":"toggle"}"#).unwrap();
        assert_eq!(r, Request::Toggle { anchor: None });
        let s = serde_json::to_string(&Request::Subscribe { kind: SubKind::Bar }).unwrap();
        assert_eq!(s, r#"{"cmd":"subscribe","kind":"bar"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"connect"}"#).unwrap();
        assert_eq!(r, Request::Connect { profile: None });
        let s = serde_json::to_string(&Request::Flip).unwrap();
        assert_eq!(s, r#"{"cmd":"flip"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"ask","profile":"work"}"#).unwrap();
        assert!(matches!(r, Request::Ask { timeout: 300, .. }));
        let r: Request = serde_json::from_str(
            r#"{"cmd":"show","anchor":{"monitor":"DP-2","edge":"bottom","x":2310,"y":1428,"width":2560,"height":1440,"reserved":[0,0,0,24],"center":false}}"#,
        )
        .unwrap();
        assert!(matches!(r, Request::Show { anchor: Some(_) }));
    }

    #[test]
    fn event_shapes() {
        let e = serde_json::to_string(&Event::Hide).unwrap();
        assert_eq!(e, r#"{"event":"hide"}"#);
        let e = serde_json::to_string(&Event::Quit).unwrap();
        assert_eq!(e, r#"{"event":"quit"}"#);
        let r = serde_json::to_string(&Reply::ok()).unwrap();
        assert_eq!(r, r#"{"ok":true}"#);
    }
}
