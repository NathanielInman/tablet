//! What the daemon knows and how it changes, without D-Bus, threads,
//! sockets or the clock. The daemon tells the [`Model`] what happened (a
//! session appeared, its status changed, a check finished, you clicked
//! Connect) and carries out the [`Effect`]s that [`Model::refresh`]
//! returns.

use crate::checks::{Report, Silence};
use crate::config::{Config, Profile};
use crate::doctor::Doctor;
use crate::model::{Ask, Entry, Phase, ProfileView, Snapshot};
use crate::openvpn::{Kind, Session, Status};
use std::collections::BTreeMap;

/// How many history entries the snapshot carries.
pub const HISTORY_SHOWN: usize = 40;

/// Something the daemon has to do after a refresh.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A new line for the bar subscribers.
    Bar(String),
    /// Append to the history file.
    History(Entry),
    /// Check this profile soon: it just connected or came back.
    Check(String),
    /// An agent's request is settled: `Ok` once the profile is held,
    /// `Err(why)` when you declined or the connect failed.
    Resolve(u64, Result<(), String>),
}

/// The daemon's memory of one profile between refreshes.
#[derive(Debug, Clone, Default)]
struct Track {
    /// Not refreshed yet: what is found now was there before the daemon
    /// started, so it goes in without a history entry.
    seen: bool,
    phase: Phase,
    since: f64,
    reason: Option<String>,
    /// The session it last had.
    path: Option<String>,
    /// That session's status kind and since when, for the grace period.
    kind: Option<Kind>,
    kind_since: f64,
    /// When that session was first seen connected.
    connected_at: Option<f64>,
    /// You (or an agent you answered) asked to connect, at this time; no
    /// session has shown up yet.
    connecting: Option<f64>,
    /// `session-start` exited with an error.
    start_error: Option<String>,
    /// Sessions the profile had when the connect began: none of them is
    /// the new one (a Reconnect is still ending them).
    before: Vec<String>,
    /// You asked to disconnect: an ending session is expected.
    stopping: bool,
    checks: Option<Report>,
    /// Checks in a row in which every probe failed.
    fails: u32,
    checking: bool,
    silence: Silence,
}

#[derive(Debug, Default)]
pub struct Model {
    pub cfg: Config,
    pub cfg_error: Option<String>,
    /// Every openvpn3 session, by path.
    sessions: BTreeMap<String, Session>,
    /// Sessions known to be gone. openvpn3 never reuses a path, so a late
    /// read of one of these (started before it went) is ignored.
    tombs: std::collections::VecDeque<String>,
    tracks: BTreeMap<String, Track>,
    asks: Vec<Ask>,
    next_ask: u64,
    doctor: Doctor,
    /// Newest last.
    history: Vec<Entry>,
    openvpn3: bool,
    bar_line: String,
    pub snap: Snapshot,
}

impl Model {
    pub fn new(cfg: Config, cfg_error: Option<String>, history: Vec<Entry>) -> Model {
        let mut m = Model {
            cfg,
            cfg_error,
            history,
            ..Default::default()
        };
        m.sync_tracks();
        m
    }

    pub fn bar_line(&self) -> &str {
        &self.bar_line
    }

    fn sync_tracks(&mut self) {
        let names: Vec<String> = self.cfg.profiles.iter().map(|p| p.name.clone()).collect();
        self.tracks.retain(|k, _| names.contains(k));
        for n in names {
            self.tracks.entry(n).or_default();
        }
    }

    /// A new config. True when it changed.
    pub fn reload(&mut self, cfg: Config, err: Option<String>) -> bool {
        if self.cfg == cfg && self.cfg_error == err {
            return false;
        }
        self.cfg = cfg;
        self.cfg_error = err;
        self.sync_tracks();
        true
    }

    /// Whether openvpn3's session manager is on the bus.
    pub fn set_openvpn3(&mut self, on: bool) {
        self.openvpn3 = on;
        if !on {
            self.sessions.clear();
        }
    }

    /// Every session, from a full scan.
    pub fn sessions(&mut self, list: Vec<Session>, now: f64) {
        let old: Vec<String> = self.sessions.keys().cloned().collect();
        self.sessions.clear();
        for p in old {
            if !list.iter().any(|s| s.path == p) {
                self.bury(p);
            }
        }
        for s in list {
            self.session(s, now);
        }
    }

    fn bury(&mut self, path: String) {
        if !self.tombs.contains(&path) {
            self.tombs.push_back(path);
            if self.tombs.len() > 64 {
                self.tombs.pop_front();
            }
        }
    }

    /// One session's properties, read fresh.
    pub fn session(&mut self, s: Session, now: f64) {
        if self.tombs.contains(&s.path) {
            return;
        }
        if let Some(name) = self.owner_of(&s.config_name) {
            let t = self.tracks.entry(name).or_default();
            t.silence
                .sample(now, s.stat("BYTES_IN"), s.stat("BYTES_OUT"));
        }
        self.sessions.insert(s.path.clone(), s);
    }

    /// A `StatusChange` signal. False for a session not seen yet (the
    /// daemon then reads it whole).
    pub fn status(&mut self, path: &str, status: Status) -> bool {
        match self.sessions.get_mut(path) {
            Some(s) => {
                s.status = status;
                true
            }
            None => false,
        }
    }

    /// The session manager says a session is gone.
    pub fn gone(&mut self, path: &str) {
        self.sessions.remove(path);
        self.bury(path.to_string());
    }

    pub fn known(&self, path: &str) -> bool {
        self.sessions.contains_key(path)
    }

    fn owner_of(&self, config_name: &str) -> Option<String> {
        self.cfg
            .profiles
            .iter()
            .find(|p| p.owns(config_name))
            .map(|p| p.name.clone())
    }

    /// The newest session a profile owns.
    fn session_of(&self, p: &Profile) -> Option<&Session> {
        self.sessions
            .values()
            .filter(|s| p.owns(&s.config_name))
            .max_by(|a, b| a.created.total_cmp(&b.created))
    }

    /// The session path for a profile, for Disconnect.
    pub fn path_of(&self, profile: &str) -> Option<String> {
        let p = self.cfg.profile(profile)?;
        self.session_of(p).map(|s| s.path.clone())
    }

    /// Every session path a profile owns (a failed one may linger beside a
    /// new one).
    pub fn paths_of(&self, profile: &str) -> Vec<String> {
        let Some(p) = self.cfg.profile(profile) else {
            return vec![];
        };
        self.sessions
            .values()
            .filter(|s| p.owns(&s.config_name))
            .map(|s| s.path.clone())
            .collect()
    }

    /// You asked to connect (the daemon starts `session-start`).
    pub fn connecting(&mut self, profile: &str, now: f64) {
        let before = self.paths_of(profile);
        if let Some(t) = self.tracks.get_mut(profile) {
            t.before = before;
            t.connecting = Some(now);
            t.start_error = None;
            t.stopping = false;
            t.fails = 0;
            t.silence.reset();
        }
    }

    /// `session-start` failed before a session showed up.
    pub fn start_failed(&mut self, profile: &str, why: String) {
        if let Some(t) = self.tracks.get_mut(profile) {
            t.start_error = Some(why);
        }
    }

    /// `session-start` finished; the session is the bus's business now.
    pub fn start_done(&mut self, profile: &str) {
        if let Some(t) = self.tracks.get_mut(profile)
            && t.start_error.is_none()
            && t.path.is_some()
        {
            t.connecting = None;
        }
    }

    /// You asked to disconnect (or dismissed a dead session).
    pub fn stopping(&mut self, profile: &str) {
        if let Some(t) = self.tracks.get_mut(profile) {
            t.stopping = true;
            t.connecting = None;
        }
    }

    /// You dismissed a red lamp.
    pub fn ack(&mut self, profile: &str) {
        if let Some(t) = self.tracks.get_mut(profile) {
            t.stopping = true;
            t.connecting = None;
            t.start_error = None;
            if matches!(t.phase, Phase::Dropped | Phase::Failed) {
                t.phase = Phase::Off;
                t.reason = None;
            }
        }
    }

    pub fn checking(&mut self, profile: &str, on: bool) {
        if let Some(t) = self.tracks.get_mut(profile) {
            t.checking = on;
        }
    }

    /// A check finished.
    pub fn checked(&mut self, profile: &str, r: Report) {
        if let Some(t) = self.tracks.get_mut(profile) {
            t.checking = false;
            if r.all_failed() {
                t.fails += 1;
            } else if !r.probes.is_empty() {
                t.fails = 0;
            }
            t.checks = Some(r);
        }
    }

    /// Consecutive failed checks, so the daemon can check again sooner.
    pub fn fails(&self, profile: &str) -> u32 {
        self.tracks.get(profile).map(|t| t.fails).unwrap_or(0)
    }

    pub fn set_doctor(&mut self, d: Doctor, now: f64) {
        let was = self.doctor.broken();
        if d.broken() != was && (d.broken() || self.doctor.at > 0.0) {
            self.history.push(Entry {
                at: now,
                profile: String::new(),
                what: if d.broken() { "broken" } else { "repaired" }.into(),
                detail: d.summary(),
            });
        }
        self.doctor = d;
    }

    pub fn doctor(&self) -> &Doctor {
        &self.doctor
    }

    /// An agent's request; returns its id.
    pub fn ask(&mut self, mut a: Ask) -> u64 {
        self.next_ask += 1;
        a.id = self.next_ask;
        let id = a.id;
        self.asks.push(a);
        id
    }

    /// You answered a request: the profile to connect, when you said yes.
    pub fn answer(&mut self, id: u64, connect: bool) -> Option<String> {
        let i = self.asks.iter().position(|a| a.id == id)?;
        if connect {
            self.asks[i].accepted = true;
            Some(self.asks[i].profile.clone())
        } else {
            None
        }
    }

    /// Drops a request (declined, or its agent stopped waiting).
    pub fn drop_ask(&mut self, id: u64) {
        self.asks.retain(|a| a.id != id);
    }

    pub fn ask_ids(&self) -> Vec<u64> {
        self.asks.iter().map(|a| a.id).collect()
    }

    /// The profile's phase as of the last refresh.
    pub fn phase(&self, profile: &str) -> Phase {
        self.tracks
            .get(profile)
            .map(|t| t.phase)
            .unwrap_or_default()
    }

    /// Recomputes every profile's phase and the snapshot, and says what the
    /// daemon must do about it.
    pub fn refresh(&mut self, now: f64) -> Vec<Effect> {
        let mut effects = vec![];
        let profiles = self.cfg.profiles.clone();
        let mut views = vec![];
        let default = self.cfg.default_profile().map(|p| p.name.clone());
        for p in &profiles {
            // While a new session is being started, the ones from before
            // (ending, or failed) are not it.
            let before = self
                .tracks
                .get(&p.name)
                .filter(|t| t.connecting.is_some())
                .map(|t| t.before.clone())
                .unwrap_or_default();
            let session = self
                .sessions
                .values()
                .filter(|s| p.owns(&s.config_name) && !before.contains(&s.path))
                .max_by(|a, b| a.created.total_cmp(&b.created))
                .cloned();
            let silence_limit = self.cfg.checks.silence as f64;
            let grace = self.cfg.checks.grace as f64;
            let start_timeout = self.cfg.openvpn3.start_timeout as f64 + 10.0;
            let t = self.tracks.entry(p.name.clone()).or_default();
            let (phase, reason) = derive(
                t,
                session.as_ref(),
                now,
                grace,
                silence_limit,
                start_timeout,
            );

            // Remember the session and when it connected.
            match &session {
                Some(s) => {
                    if t.path.as_deref() != Some(&s.path) {
                        t.path = Some(s.path.clone());
                        t.connected_at = None;
                        t.fails = 0;
                        t.checks = None;
                        t.kind = None;
                    }
                    let kind = s.status.kind();
                    if t.kind != Some(kind) {
                        t.kind = Some(kind);
                        t.kind_since = now;
                    }
                    if kind == Kind::Connected && t.connected_at.is_none() {
                        // Seen connected for the first time: if Tablet was
                        // just started, the session's age is the best guess.
                        t.connected_at =
                            Some(if (!t.seen || t.phase == Phase::Off) && s.created > 0.0 {
                                s.created
                            } else {
                                now
                            });
                    }
                    if kind != Kind::Starting || s.failure().is_some() {
                        t.connecting = None;
                        t.before.clear();
                    }
                }
                None => {
                    if t.path.take().is_some() {
                        t.kind = None;
                        t.connected_at = None;
                        t.checks = None;
                        t.fails = 0;
                        t.silence.reset();
                    }
                }
            }
            // A disconnect you asked for is over once its session is gone.
            if session.is_none() && matches!(phase, Phase::Off | Phase::Dropped) {
                t.stopping = false;
            }

            if phase != t.phase {
                if t.seen
                    && let Some(e) = history_for(
                        &p.name,
                        t.phase,
                        phase,
                        reason.as_deref(),
                        session.as_ref(),
                        now,
                    )
                {
                    effects.push(Effect::History(e.clone()));
                    self.history.push(e);
                }
                if phase == Phase::Held && t.phase != Phase::Stale {
                    effects.push(Effect::Check(p.name.clone()));
                }
                t.phase = phase;
                t.since = now;
            }
            t.seen = true;
            t.reason = reason.clone();

            let busy = if t.checking {
                Some("checking".to_string())
            } else if t.stopping && session.is_some() {
                Some("disconnecting".to_string())
            } else if t.connecting.is_some() && session.is_none() {
                Some("connecting".to_string())
            } else {
                None
            };
            views.push(ProfileView {
                name: p.name.clone(),
                default: default.as_deref() == Some(&p.name),
                ovpn: crate::paths::tilde(&p.ovpn_path()),
                phase,
                since: t.since,
                connected_at: if phase.up() { t.connected_at } else { None },
                reason,
                signin_url: session.as_ref().and_then(|s| s.status.url()),
                session,
                checks: t.checks.clone(),
                silent_for: t.silence.silent_for(now).filter(|s| *s >= 15.0),
                busy,
            });
        }

        // Requests settle once their profile is held, or fail with it.
        let mut settled = vec![];
        for a in &self.asks {
            let Some(v) = views.iter().find(|v| v.name == a.profile) else {
                settled.push((a.id, Err(format!("there is no profile `{}`", a.profile))));
                continue;
            };
            if v.phase == Phase::Held {
                settled.push((a.id, Ok(())));
            } else if a.accepted && v.phase == Phase::Failed && v.since >= a.at {
                settled.push((
                    a.id,
                    Err(format!(
                        "{} failed to connect: {}",
                        a.profile,
                        v.reason.clone().unwrap_or_default()
                    )),
                ));
            }
        }
        for (id, r) in settled {
            self.asks.retain(|a| a.id != id);
            effects.push(Effect::Resolve(id, r));
        }

        let others: Vec<Session> = self
            .sessions
            .values()
            .filter(|s| !profiles.iter().any(|p| p.owns(&s.config_name)))
            .cloned()
            .collect();
        let keep = self.history.len().saturating_sub(HISTORY_SHOWN);
        let mut snap = Snapshot {
            version: env!("CARGO_PKG_VERSION").into(),
            now,
            lamp: String::new(),
            profiles: views,
            others,
            asks: self.asks.clone(),
            doctor: self.doctor.clone(),
            history: self.history[keep..].iter().rev().cloned().collect(),
            openvpn3: self.openvpn3,
            config_error: self.cfg_error.clone(),
        };
        if self.history.len() > HISTORY_SHOWN * 4 {
            self.history.drain(..self.history.len() - HISTORY_SHOWN);
        }
        snap.lamp = crate::bar::lamp(&snap).to_string();
        let line = serde_json::to_string(&crate::bar::line(&snap, &self.cfg)).unwrap_or_default();
        if line != self.bar_line {
            self.bar_line = line.clone();
            effects.push(Effect::Bar(line));
        }
        self.snap = snap;
        effects
    }
}

/// A profile's phase and why, from its session and memory.
fn derive(
    t: &Track,
    s: Option<&Session>,
    now: f64,
    grace: f64,
    silence_limit: f64,
    start_timeout: f64,
) -> (Phase, Option<String>) {
    // The same session as last time (its status may have moved on).
    let same = |s: &Session| t.path.as_deref() == Some(&s.path);
    let Some(s) = s else {
        if let Some(at) = t.connecting {
            if let Some(e) = &t.start_error {
                return (Phase::Failed, Some(e.clone()));
            }
            if now - at > start_timeout {
                return (
                    Phase::Failed,
                    Some("openvpn3 never started a session".into()),
                );
            }
            return (Phase::Starting, Some("starting openvpn3".into()));
        }
        if t.stopping {
            return (Phase::Off, None);
        }
        return match t.phase {
            // A session that was up (or on its way) went away by itself.
            Phase::Held | Phase::Stale | Phase::Reconnecting | Phase::SignIn | Phase::Starting
                if t.path.is_some() =>
            {
                (
                    Phase::Dropped,
                    Some("the session ended without being asked to".into()),
                )
            }
            // Red stays red until you act.
            Phase::Dropped | Phase::Failed => (t.phase, t.reason.clone()),
            _ => (Phase::Off, None),
        };
    };
    let kind = s.status.kind();
    // Dismissed or disconnected on purpose: what is left of it is going.
    if t.stopping && (matches!(kind, Kind::Failed | Kind::Ended) || s.failure().is_some()) {
        return (Phase::Off, None);
    }
    let kind_since = if same(s) && t.kind == Some(kind) {
        t.kind_since
    } else {
        now
    };
    let was_up = same(s) && t.connected_at.is_some();
    match kind {
        Kind::Connected => {
            if t.fails >= 2 {
                let n = t.checks.as_ref().map(|c| c.probes.len()).unwrap_or(0);
                let what = if n == 1 {
                    "the probe".to_string()
                } else {
                    format!("all {n} probes")
                };
                return (
                    Phase::Stale,
                    Some(format!(
                        "openvpn3 says connected, but {what} failed twice in a row"
                    )),
                );
            }
            if let Some(quiet) = t.silence.silent_for(now)
                && silence_limit > 0.0
                && quiet >= silence_limit
            {
                return (
                    Phase::Stale,
                    Some(format!(
                        "the server has sent nothing for {} while traffic kept going out",
                        crate::text::span(quiet)
                    )),
                );
            }
            (Phase::Held, None)
        }
        Kind::SignIn => (Phase::SignIn, Some(s.status.describe())),
        Kind::Failed => (Phase::Failed, s.failure()),
        Kind::Ended => (
            Phase::Dropped,
            Some(format!("openvpn3: {}", s.status.describe())),
        ),
        Kind::Paused => (Phase::Reconnecting, Some("paused".into())),
        Kind::Starting | Kind::Reconnecting => {
            if let Some(why) = s.failure() {
                return (Phase::Failed, Some(why));
            }
            let waited = now - kind_since;
            if grace > 0.0 && waited > grace {
                let doing = if was_up || kind == Kind::Reconnecting {
                    "reconnecting"
                } else {
                    "connecting"
                };
                return (
                    Phase::Failed,
                    Some(format!("still {doing} after {}", crate::text::span(waited))),
                );
            }
            if was_up || kind == Kind::Reconnecting {
                (Phase::Reconnecting, Some(s.status.describe()))
            } else {
                (Phase::Starting, Some(s.status.describe()))
            }
        }
    }
}

fn history_for(
    profile: &str,
    from: Phase,
    to: Phase,
    reason: Option<&str>,
    s: Option<&Session>,
    now: f64,
) -> Option<Entry> {
    let server = s.map(|s| s.server.clone()).unwrap_or_default();
    let (what, detail) = match (from, to) {
        (Phase::Reconnecting, Phase::Held) => ("reconnected", server),
        (Phase::Stale, Phase::Held) => ("recovered", "the tunnel answers again".to_string()),
        (_, Phase::Held) => ("connected", server),
        (_, Phase::Stale) => ("stale", reason.unwrap_or_default().to_string()),
        (Phase::Held | Phase::Stale, Phase::Reconnecting) => {
            ("reconnecting", reason.unwrap_or_default().to_string())
        }
        (_, Phase::Failed) => ("failed", reason.unwrap_or_default().to_string()),
        (_, Phase::Dropped) => ("dropped", reason.unwrap_or_default().to_string()),
        (_, Phase::SignIn) => ("signin", reason.unwrap_or_default().to_string()),
        (
            Phase::Held | Phase::Stale | Phase::Reconnecting | Phase::SignIn | Phase::Starting,
            Phase::Off,
        ) => ("disconnected", String::new()),
        _ => return None,
    };
    Some(Entry {
        at: now,
        profile: profile.to_string(),
        what: what.into(),
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openvpn::minor;

    fn cfg() -> Config {
        Config::parse(
            r#"
[checks]
grace = 60
silence = 90

[[profile]]
name = "work"
ovpn = "/home/user/work.ovpn"
probes = ["git.corp.example.com:22", "wiki.corp.example.com"]
"#,
        )
        .unwrap()
    }

    fn session(path: &str, minor: u32, created: f64) -> Session {
        Session {
            path: path.into(),
            config_name: "/home/user/work.ovpn".into(),
            server: "vpn.example.com".into(),
            device: "tun0".into(),
            created,
            status: Status {
                major: 2,
                minor,
                message: String::new(),
            },
            ..Default::default()
        }
    }

    fn phase(m: &Model) -> Phase {
        m.snap.profile("work").unwrap().phase
    }

    fn whats(e: &[Effect]) -> Vec<String> {
        e.iter()
            .filter_map(|e| match e {
                Effect::History(h) => Some(h.what.clone()),
                _ => None,
            })
            .collect()
    }

    fn report(ok: bool) -> Report {
        let p = |t: &str| crate::checks::Probe {
            target: t.into(),
            ok,
            ms: 30,
            error: (!ok).then(|| "timed out".into()),
        };
        Report {
            at: 0.0,
            probes: vec![p("git.corp.example.com:22"), p("wiki.corp.example.com")],
            ..Default::default()
        }
    }

    #[test]
    fn what_was_there_at_start_is_not_news() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 10.0)], 50.0);
        let e = m.refresh(50.0);
        assert_eq!(phase(&m), Phase::Held);
        assert!(whats(&e).is_empty());
        assert!(e.contains(&Effect::Check("work".into())));
    }

    #[test]
    fn a_click_connects_through_sign_in() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.refresh(0.0);
        assert_eq!(phase(&m), Phase::Off);
        m.connecting("work", 1.0);
        m.refresh(1.0);
        assert_eq!(phase(&m), Phase::Starting);
        assert_eq!(m.snap.lamp, "connecting");
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTING, 2.0)], 2.0);
        m.refresh(2.0);
        assert_eq!(phase(&m), Phase::Starting);
        let mut s = session("/s/1", minor::SESS_AUTH_URL, 2.0);
        s.status.message = "https://login.example.com/x".into();
        m.session(s, 3.0);
        let e = m.refresh(3.0);
        assert_eq!(phase(&m), Phase::SignIn);
        assert_eq!(whats(&e), vec!["signin"]);
        assert_eq!(m.snap.lamp, "signin");
        assert_eq!(
            m.snap.profile("work").unwrap().signin_url.as_deref(),
            Some("https://login.example.com/x")
        );
        assert!(m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_CONNECTED,
                message: String::new()
            }
        ));
        let e = m.refresh(20.0);
        assert_eq!(phase(&m), Phase::Held);
        assert_eq!(whats(&e), vec!["connected"]);
        assert!(e.contains(&Effect::Check("work".into())));
        assert_eq!(m.snap.profile("work").unwrap().connected_at, Some(20.0));
        assert_eq!(m.snap.lamp, "held");
    }

    #[test]
    fn a_session_found_at_start_counts_from_its_creation() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 1000.0)], 5000.0);
        m.refresh(5000.0);
        assert_eq!(phase(&m), Phase::Held);
        assert_eq!(m.snap.profile("work").unwrap().connected_at, Some(1000.0));
    }

    #[test]
    fn a_session_that_vanishes_is_dropped_until_acknowledged() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 0.0)], 0.0);
        m.refresh(0.0);
        m.gone("/s/1");
        let e = m.refresh(10.0);
        assert_eq!(phase(&m), Phase::Dropped);
        assert_eq!(whats(&e), vec!["dropped"]);
        assert_eq!(m.snap.lamp, "dropped");
        // Still red later.
        m.refresh(500.0);
        assert_eq!(phase(&m), Phase::Dropped);
        m.ack("work");
        m.refresh(501.0);
        assert_eq!(phase(&m), Phase::Off);
        assert_eq!(m.snap.lamp, "off");
    }

    #[test]
    fn a_disconnect_you_asked_for_is_not_a_drop() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 0.0)], 0.0);
        m.refresh(0.0);
        m.stopping("work");
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_DISCONNECTED,
                message: String::new(),
            },
        );
        m.refresh(1.0);
        assert_eq!(phase(&m), Phase::Off);
        // Still listed a moment later, still not a drop.
        m.refresh(1.5);
        assert_eq!(phase(&m), Phase::Off);
        m.gone("/s/1");
        m.refresh(1.8);
        // A read that started before it went comes back late: ignored.
        let mut late = session("/s/1", minor::CONN_DISCONNECTED, 0.0);
        late.status.minor = minor::CONN_DISCONNECTED;
        m.session(late, 1.9);
        let e = m.refresh(2.0);
        assert_eq!(phase(&m), Phase::Off);
        assert!(whats(&e).is_empty());
    }

    #[test]
    fn a_refused_sign_in_that_lingers_is_failed() {
        // The session from Nov 11: CONN_CONNECTING with AUTH_FAILED = 1,
        // for a day, while the old lamp said connected.
        let mut m = Model::new(cfg(), None, vec![]);
        m.refresh(0.0);
        let mut s = session("/s/1", minor::CONN_CONNECTING, 0.0);
        s.stats.insert("AUTH_FAILED".into(), 1);
        m.sessions(vec![s], 0.0);
        let e = m.refresh(0.0);
        assert_eq!(phase(&m), Phase::Failed);
        assert_eq!(whats(&e), vec!["failed"]);
        assert!(
            m.snap
                .profile("work")
                .unwrap()
                .reason
                .as_deref()
                .unwrap()
                .contains("refused")
        );
        assert_eq!(m.snap.lamp, "failed");
        // Dismissed: the dead session is being disconnected, so it is off.
        m.ack("work");
        let e = m.refresh(1.0);
        assert_eq!(phase(&m), Phase::Off);
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_DISCONNECTED,
                message: String::new(),
            },
        );
        let e2 = m.refresh(1.2);
        assert_eq!(phase(&m), Phase::Off);
        assert!(whats(&e).is_empty() && whats(&e2).is_empty());
    }

    #[test]
    fn connecting_too_long_fails() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTING, 0.0)], 0.0);
        m.refresh(0.0);
        assert_eq!(phase(&m), Phase::Starting);
        m.refresh(61.0);
        assert_eq!(phase(&m), Phase::Failed);
        assert!(
            m.snap
                .profile("work")
                .unwrap()
                .reason
                .as_deref()
                .unwrap()
                .starts_with("still connecting")
        );
    }

    #[test]
    fn a_quick_connect_after_a_disconnect_ignores_the_old_session() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 100.0)], 100.0);
        m.refresh(100.0);
        m.stopping("work");
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_DISCONNECTED,
                message: String::new(),
            },
        );
        m.refresh(100.5);
        assert_eq!(phase(&m), Phase::Off);
        // Connect again while the old one is still listed as ending.
        m.connecting("work", 100.8);
        let e = m.refresh(100.8);
        assert_eq!(phase(&m), Phase::Starting);
        assert!(whats(&e).is_empty());
        m.sessions(
            vec![
                session("/s/1", minor::CONN_DISCONNECTED, 100.0),
                session("/s/2", minor::CONN_CONNECTING, 101.0),
            ],
            101.0,
        );
        m.refresh(101.0);
        assert_eq!(phase(&m), Phase::Starting);
        m.sessions(vec![session("/s/2", minor::CONN_CONNECTED, 101.0)], 102.0);
        m.refresh(102.0);
        assert_eq!(phase(&m), Phase::Held);
    }

    #[test]
    fn a_new_session_that_fails_at_once_is_failed() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.refresh(0.0);
        m.connecting("work", 1.0);
        m.sessions(vec![session("/s/1", minor::CONN_AUTH_FAILED, 1.5)], 2.0);
        m.refresh(2.0);
        assert_eq!(phase(&m), Phase::Failed);
    }

    #[test]
    fn reconnects_by_itself() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 0.0)], 0.0);
        m.refresh(0.0);
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_RECONNECTING,
                message: String::new(),
            },
        );
        let e = m.refresh(100.0);
        assert_eq!(phase(&m), Phase::Reconnecting);
        assert_eq!(whats(&e), vec!["reconnecting"]);
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_CONNECTED,
                message: String::new(),
            },
        );
        let e = m.refresh(109.0);
        assert_eq!(phase(&m), Phase::Held);
        assert_eq!(whats(&e), vec!["reconnected"]);
        // Stuck reconnecting past the grace period.
        m.status(
            "/s/1",
            Status {
                major: 2,
                minor: minor::CONN_RECONNECTING,
                message: String::new(),
            },
        );
        m.refresh(200.0);
        m.refresh(261.0);
        assert_eq!(phase(&m), Phase::Failed);
    }

    #[test]
    fn probes_failing_twice_make_it_stale() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 0.0)], 0.0);
        m.refresh(0.0);
        m.checked("work", report(false));
        m.refresh(10.0);
        assert_eq!(phase(&m), Phase::Held, "one bad check is not enough");
        assert_eq!(m.fails("work"), 1);
        m.checked("work", report(false));
        let e = m.refresh(20.0);
        assert_eq!(phase(&m), Phase::Stale);
        assert_eq!(whats(&e), vec!["stale"]);
        assert_eq!(m.snap.lamp, "stale");
        m.checked("work", report(true));
        let e = m.refresh(30.0);
        assert_eq!(phase(&m), Phase::Held);
        assert_eq!(whats(&e), vec!["recovered"]);
        // Coming back from stale does not ask for another check.
        assert!(!e.contains(&Effect::Check("work".into())));
    }

    #[test]
    fn a_silent_server_makes_it_stale() {
        let mut m = Model::new(cfg(), None, vec![]);
        let mut s = session("/s/1", minor::CONN_CONNECTED, 0.0);
        s.stats.insert("BYTES_IN".into(), 1000);
        s.stats.insert("BYTES_OUT".into(), 1000);
        m.sessions(vec![s.clone()], 0.0);
        m.refresh(0.0);
        s.stats.insert("BYTES_OUT".into(), 4000);
        m.session(s.clone(), 60.0);
        m.refresh(60.0);
        assert_eq!(phase(&m), Phase::Held);
        s.stats.insert("BYTES_OUT".into(), 9000);
        m.session(s.clone(), 100.0);
        m.refresh(100.0);
        assert_eq!(phase(&m), Phase::Stale);
        assert!(
            m.snap
                .profile("work")
                .unwrap()
                .reason
                .as_deref()
                .unwrap()
                .contains("sent nothing for 1m")
        );
        s.stats.insert("BYTES_IN".into(), 1200);
        m.session(s, 110.0);
        m.refresh(110.0);
        assert_eq!(phase(&m), Phase::Held);
    }

    #[test]
    fn a_failed_start_says_why() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.connecting("work", 0.0);
        m.start_failed("work", "New tunnel did not respond".into());
        m.refresh(1.0);
        assert_eq!(phase(&m), Phase::Failed);
        assert_eq!(
            m.snap.profile("work").unwrap().reason.as_deref(),
            Some("New tunnel did not respond")
        );
    }

    #[test]
    fn asks_settle_when_the_profile_is_held() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.refresh(0.0);
        let id = m.ask(Ask {
            profile: "work".into(),
            reason: "nexus".into(),
            at: 0.0,
            ..Default::default()
        });
        m.refresh(1.0);
        assert_eq!(m.snap.lamp, "asking");
        assert_eq!(m.answer(id, true).as_deref(), Some("work"));
        m.connecting("work", 2.0);
        m.sessions(vec![session("/s/1", minor::CONN_CONNECTED, 3.0)], 3.0);
        let e = m.refresh(3.0);
        assert!(e.contains(&Effect::Resolve(id, Ok(()))));
        assert!(m.snap.asks.is_empty());

        let bad = m.ask(Ask {
            profile: "nope".into(),
            ..Default::default()
        });
        let e = m.refresh(4.0);
        assert!(matches!(
            e.iter()
                .find(|x| matches!(x, Effect::Resolve(i, _) if *i == bad)),
            Some(Effect::Resolve(_, Err(_)))
        ));
    }

    #[test]
    fn a_broken_openvpn3_shows_on_the_lamp() {
        let mut m = Model::new(cfg(), None, vec![]);
        m.set_doctor(
            Doctor {
                at: 1.0,
                checked: 3,
                missing: vec![("/usr/bin/openvpn3".into(), "libprotobuf.so.36.0.0".into())],
                ..Default::default()
            },
            1.0,
        );
        let e = m.refresh(1.0);
        assert_eq!(m.snap.lamp, "broken");
        assert!(
            e.iter()
                .any(|x| matches!(x, Effect::Bar(l) if l.contains("broken")))
        );
        assert_eq!(m.snap.history[0].what, "broken");
    }

    #[test]
    fn sessions_of_other_configs_are_listed_apart() {
        let mut m = Model::new(cfg(), None, vec![]);
        let mut s = session("/s/9", minor::CONN_CONNECTED, 0.0);
        s.config_name = "/home/user/other.ovpn".into();
        m.sessions(vec![s], 0.0);
        m.refresh(0.0);
        assert_eq!(phase(&m), Phase::Off);
        assert_eq!(m.snap.others.len(), 1);
    }
}
