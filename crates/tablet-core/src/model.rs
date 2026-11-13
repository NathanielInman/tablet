//! What the daemon knows, as the drawer receives it and `tablet status
//! --json` prints it.

use crate::checks::Report;
use crate::doctor::Doctor;
use crate::openvpn::Session;
use serde::{Deserialize, Serialize};

/// Where a profile's tunnel stands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No session, and none expected.
    #[default]
    Off,
    /// A session is on its way up.
    Starting,
    /// openvpn3 waits for a web sign-in, a password or a challenge.
    #[serde(rename = "signin")]
    SignIn,
    /// Connected, and the checks pass (or there are none yet).
    Held,
    /// Was connected; openvpn3 lost the server and is trying again, or the
    /// session is paused.
    Reconnecting,
    /// openvpn3 says connected, but nothing gets through: every probe failed
    /// twice in a row, or the server has gone silent.
    Stale,
    /// The session gave up (a refused sign-in, a failed connection, a bad
    /// config) or never started. It stays red until you act.
    Failed,
    /// A held session went away without being asked to. It stays red until
    /// you reconnect or dismiss it.
    Dropped,
}

impl Phase {
    /// As the JSON and the bar's classes spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Starting => "starting",
            Phase::SignIn => "signin",
            Phase::Held => "held",
            Phase::Reconnecting => "reconnecting",
            Phase::Stale => "stale",
            Phase::Failed => "failed",
            Phase::Dropped => "dropped",
        }
    }

    /// In words, for the drawer and the CLI.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Starting => "connecting",
            Phase::SignIn => "sign in",
            Phase::Held => "held",
            Phase::Reconnecting => "reconnecting",
            Phase::Stale => "stale",
            Phase::Failed => "failed",
            Phase::Dropped => "dropped",
        }
    }

    /// Needs the person: red or amber, never calm.
    pub fn trouble(self) -> bool {
        matches!(self, Phase::Stale | Phase::Failed | Phase::Dropped)
    }

    /// A session exists and is (or was just) carrying traffic.
    pub fn up(self) -> bool {
        matches!(self, Phase::Held | Phase::Stale | Phase::Reconnecting)
    }
}

/// One profile as the drawer shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProfileView {
    pub name: String,
    /// What a click on the lamp connects.
    pub default: bool,
    /// The `.ovpn` file, as `~/...`.
    pub ovpn: String,
    pub phase: Phase,
    /// Unix seconds when it entered this phase.
    pub since: f64,
    /// Unix seconds when the current session connected (the session's
    /// creation, for one that was connected before Tablet started).
    pub connected_at: Option<f64>,
    /// Why it is stale, failed or dropped, or what openvpn3 is doing.
    pub reason: Option<String>,
    pub session: Option<Session>,
    /// The last check.
    pub checks: Option<Report>,
    /// Seconds the server has been silent while bytes went out.
    pub silent_for: Option<f64>,
    /// The web sign-in page openvpn3 is waiting on.
    pub signin_url: Option<String>,
    /// What Tablet is doing for it right now: `connecting`,
    /// `disconnecting`, `checking`.
    pub busy: Option<String>,
}

/// An agent's request for a profile (`tablet need`), waiting on you.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Ask {
    pub id: u64,
    pub profile: String,
    /// The agent's reason, unverified.
    pub reason: String,
    /// The command it wants to run.
    pub command: String,
    pub cwd: String,
    /// The program that asked (the parent of `tablet need`).
    pub requester: String,
    /// Unix seconds.
    pub at: f64,
    /// You said Connect.
    pub accepted: bool,
}

/// One line of history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Entry {
    /// Unix seconds.
    pub at: f64,
    /// The profile, or empty for openvpn3 itself.
    pub profile: String,
    /// `connected`, `reconnected`, `recovered`, `stale`, `reconnecting`,
    /// `failed`, `dropped`, `disconnected`, `signin`, `broken`, `repaired`.
    pub what: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Snapshot {
    pub version: String,
    /// Unix seconds when this was made.
    pub now: f64,
    /// The lamp's state: `off`, `held`, `degraded`, `connecting`, `stale`,
    /// `signin`, `asking`, `failed`, `dropped` or `broken`.
    pub lamp: String,
    pub profiles: Vec<ProfileView>,
    /// openvpn3 sessions no profile claims.
    pub others: Vec<Session>,
    pub asks: Vec<Ask>,
    pub doctor: Doctor,
    /// Newest first.
    pub history: Vec<Entry>,
    /// openvpn3's session manager is running (it starts on demand).
    pub openvpn3: bool,
    pub config_error: Option<String>,
}

impl Snapshot {
    pub fn profile(&self, name: &str) -> Option<&ProfileView> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn default_profile(&self) -> Option<&ProfileView> {
        self.profiles
            .iter()
            .find(|p| p.default)
            .or_else(|| self.profiles.first())
    }
}
