//! The daemon: follows openvpn3's sessions on the system bus, checks that a
//! connected tunnel actually works, keeps an eye on openvpn3's own health,
//! serves the socket, feeds the bar module, and starts the drawer the first
//! time it is asked for.
//!
//! Sessions are followed through openvpn3's signals (`SessionManagerEvent`,
//! `StatusChange`, `PropertiesChanged`) and the bus's `NameOwnerChanged`.
//! openvpn3 does not promise to send every status change to every listener,
//! so while a session exists its properties are also read every few seconds
//! (one D-Bus call each); with no session nothing is read, and openvpn3 is
//! never started just to be asked. Every D-Bus call, probe and command runs
//! on a helper thread or a client's connection thread, never under the lock
//! that feeds the bar.

use crate::dbus;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tablet_core::checks::{self, Report};
use tablet_core::config::Config;
use tablet_core::model::{Ask, Entry, Phase};
use tablet_core::openvpn::{BUS, IFACE, MANAGER_PATH, Status};
use tablet_core::protocol::{Event, Reply, Request, SubKind};
use tablet_core::state::{Effect, Model};
use tablet_core::transport::{Listener, Stream};
use tablet_core::{doctor, now, paths};
use zbus::message::Type as MsgType;
use zbus::{Connection, MatchRule};

/// How often a session's properties are read while one exists.
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// How long after connecting (or coming back) the first check waits, so
/// the tunnel's routes and DNS are in place.
const FIRST_CHECK: Duration = Duration::from_secs(4);
/// How soon a failed check is repeated before the tunnel counts as stale.
const RECHECK: Duration = Duration::from_secs(15);
/// How often `/etc/ld.so.cache` is looked at for a library update.
const DOCTOR_EVERY: Duration = Duration::from_secs(60);
/// History lines kept on disk.
const HISTORY_KEEP: usize = 500;

struct State {
    m: Model,
    boards: Vec<Sub>,
    bars: Vec<Sub>,
    /// A show the drawer has not received yet because it is still starting.
    pending: Option<Event>,
    board_started: Option<Instant>,
    /// When each profile is due a check.
    due: HashMap<String, Instant>,
    /// Settled agent requests, waiting for their connection to pick them up.
    resolved: HashMap<u64, Result<(), String>>,
    /// Profiles whose `session-start` is running.
    starting: Vec<String>,
}

pub struct Daemon {
    state: Mutex<State>,
    cv: Condvar,
    conn: Connection,
}

type Shared = Arc<Daemon>;

impl Daemon {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn send_line(s: &mut Stream, line: &str) -> bool {
    s.write_all(line.as_bytes())
        .and_then(|_| s.write_all(b"\n"))
        .and_then(|_| s.flush())
        .is_ok()
}

/// Lines a subscriber may fall behind by before it counts as stuck.
const SUB_QUEUE: usize = 64;

/// A bar or drawer subscription. Its own thread writes to the socket, so a
/// reader that stops reading never holds up the state lock.
struct Sub {
    tx: std::sync::mpsc::SyncSender<String>,
    writer: std::thread::JoinHandle<()>,
}

impl Sub {
    fn new(mut s: Stream) -> Sub {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(SUB_QUEUE);
        let writer = std::thread::spawn(move || {
            for line in rx {
                if !send_line(&mut s, &line) {
                    break;
                }
            }
        });
        Sub { tx, writer }
    }

    fn send(&self, line: &str) -> bool {
        self.tx.try_send(line.to_string()).is_ok()
    }

    fn finish(self) {
        let Sub { tx, writer } = self;
        drop(tx);
        let _ = writer.join();
    }
}

fn broadcast(subs: &mut Vec<Sub>, line: &str) -> usize {
    subs.retain(|s| s.send(line));
    subs.len()
}

fn event_line(e: &Event) -> String {
    serde_json::to_string(e).unwrap_or_default()
}

fn load_history() -> Vec<Entry> {
    let text = std::fs::read_to_string(paths::history_file()).unwrap_or_default();
    let mut v: Vec<Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if v.len() > HISTORY_KEEP {
        v.drain(..v.len() - HISTORY_KEEP);
        // Rewrite the file shorter, once, at start.
        let body: String = v
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|l| l + "\n")
            .collect();
        let _ = std::fs::write(paths::history_file(), body);
    }
    v
}

fn append_history(e: &Entry) {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::create_dir_all(paths::state_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths::history_file())
        && let Ok(line) = serde_json::to_string(e)
    {
        let _ = writeln!(f, "{line}");
    }
    eprintln!(
        "tablet: {}{}{}",
        if e.profile.is_empty() {
            String::new()
        } else {
            format!("{}: ", e.profile)
        },
        e.what,
        if e.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", e.detail)
        }
    );
}

/// Tells the drawer to exit and exits: a restart, or an upgrade replaced
/// this binary. Bars see their stream end and start the new daemon.
fn retire(st: &mut State) -> ! {
    let quit = event_line(&Event::Quit);
    for b in std::mem::take(&mut st.boards) {
        b.send(&quit);
        b.finish();
    }
    eprintln!("tablet: exiting");
    std::process::exit(0)
}

/// Recomputes the snapshot and pushes whatever changed.
fn refresh(d: &Shared, st: &mut State) {
    if paths::exe_replaced() {
        retire(st);
    }
    for e in st.m.refresh(now()) {
        match e {
            Effect::Bar(line) => {
                broadcast(&mut st.bars, &line);
            }
            Effect::History(h) => append_history(&h),
            Effect::Check(p) => {
                st.due.insert(p, Instant::now() + FIRST_CHECK);
                d.cv.notify_all();
            }
            Effect::Resolve(id, r) => {
                st.resolved.insert(id, r);
                d.cv.notify_all();
            }
        }
    }
    let ev = event_line(&Event::Snapshot {
        snapshot: Box::new(st.m.snap.clone()),
    });
    broadcast(&mut st.boards, &ev);
}

/// Reads every session again. Never starts openvpn3: with nobody owning its
/// name there is nothing to read.
fn rescan(d: &Shared) {
    let alive = dbus::has_owner(&d.conn, BUS);
    let list = if alive {
        match dbus::sessions(&d.conn) {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("tablet: reading openvpn3's sessions: {e:#}");
                None
            }
        }
    } else {
        Some(vec![])
    };
    let mut st = d.lock();
    st.m.set_openvpn3(alive);
    if let Some(l) = list {
        st.m.sessions(l, now());
    }
    refresh(d, &mut st);
}

/// Reads one session again (after a signal about it).
fn reread(d: &Shared, path: &str) {
    match dbus::session(&d.conn, path) {
        Ok(s) => {
            let mut st = d.lock();
            st.m.session(s, now());
            refresh(d, &mut st);
        }
        // Gone between the signal and the read.
        Err(_) => rescan(d),
    }
}

fn rule(iface: &str, member: &str) -> zbus::match_rule::Builder<'static> {
    MatchRule::builder()
        .msg_type(MsgType::Signal)
        .interface(iface.to_string())
        .member(member.to_string())
}

/// Sessions created and destroyed.
fn watch_manager(d: Shared) -> Result<()> {
    let r = rule(IFACE, "SessionManagerEvent")
        .path(MANAGER_PATH.to_string())
        .build()?;
    for msg in dbus::signals(&d.conn, r, 64)? {
        if msg.is_err() {
            continue;
        }
        let d = d.clone();
        std::thread::spawn(move || rescan(&d));
    }
    Err(anyhow!("the system bus closed"))
}

/// A session's status moved on.
fn watch_status(d: Shared) -> Result<()> {
    let r = rule(IFACE, "StatusChange")
        .path_namespace(MANAGER_PATH.to_string())
        .build()?;
    for msg in dbus::signals(&d.conn, r, 256)? {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        let Some(path) = header.path().map(|p| p.to_string()) else {
            continue;
        };
        if path == MANAGER_PATH {
            continue;
        }
        let Ok((major, minor, message)) = msg.body().deserialize::<(u32, u32, String)>() else {
            continue;
        };
        let mut st = d.lock();
        if st.m.status(
            &path,
            Status {
                major,
                minor,
                message,
            },
        ) {
            refresh(&d, &mut st);
            drop(st);
        } else {
            drop(st);
        }
        // The counters and device may have changed too; read it whole.
        let d = d.clone();
        std::thread::spawn(move || reread(&d, &path));
    }
    Err(anyhow!("the system bus closed"))
}

/// Property changes and sign-in prompts on a session.
fn watch_props(d: Shared) -> Result<()> {
    let r = MatchRule::builder()
        .msg_type(MsgType::Signal)
        .path_namespace(MANAGER_PATH.to_string())
        .build()?;
    for msg in dbus::signals(&d.conn, r, 256)? {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        let member = header.member().map(|m| m.to_string()).unwrap_or_default();
        if !matches!(member.as_str(), "PropertiesChanged" | "AttentionRequired") {
            continue;
        }
        let Some(path) = header.path().map(|p| p.to_string()) else {
            continue;
        };
        if path == MANAGER_PATH {
            continue;
        }
        let d = d.clone();
        std::thread::spawn(move || reread(&d, &path));
    }
    Err(anyhow!("the system bus closed"))
}

/// openvpn3's session manager starting or stopping.
fn watch_names(d: Shared) -> Result<()> {
    let r = rule("org.freedesktop.DBus", "NameOwnerChanged")
        .path("/org/freedesktop/DBus")
        .build()?;
    for msg in dbus::signals(&d.conn, r, 64)? {
        let Ok(msg) = msg else { continue };
        let Ok((name, _, _)) = msg.body().deserialize::<(String, String, String)>() else {
            continue;
        };
        if name == BUS {
            let d = d.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                rescan(&d);
            });
        }
    }
    Err(anyhow!("the system bus closed"))
}

/// Keeps a watcher alive; exits when the bus itself is gone.
fn keep(d: &Shared, name: &'static str, f: fn(Shared) -> Result<()>) {
    let d = d.clone();
    std::thread::spawn(move || {
        loop {
            if let Err(e) = f(d.clone()) {
                eprintln!("tablet: {name} watcher: {e:#}");
            }
            if !dbus::bus_alive(&d.conn) {
                eprintln!("tablet: the system bus is gone");
                retire(&mut d.lock());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Seconds of `clock`.
fn clock_secs(clock: libc::clockid_t) -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(clock, &mut ts);
    }
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// Reads sessions every few seconds while one exists (for the counters and
/// any status change openvpn3 did not signal), refreshes the timers
/// (grace periods), and notices a resume from suspend: the boot clock
/// jumps while the monotonic clock does not, and every tunnel is checked
/// a few seconds later.
fn watcher(d: Shared) {
    let mut last = (
        clock_secs(libc::CLOCK_BOOTTIME),
        clock_secs(libc::CLOCK_MONOTONIC),
    );
    loop {
        std::thread::sleep(WATCH_EVERY);
        let now_clocks = (
            clock_secs(libc::CLOCK_BOOTTIME),
            clock_secs(libc::CLOCK_MONOTONIC),
        );
        let slept = (now_clocks.0 - last.0) - (now_clocks.1 - last.1);
        last = now_clocks;
        if slept > 10.0 {
            eprintln!("tablet: resumed after {:.0}s asleep; checking", slept);
            let mut st = d.lock();
            let up: Vec<String> =
                st.m.snap
                    .profiles
                    .iter()
                    .filter(|p| p.phase.up())
                    .map(|p| p.name.clone())
                    .collect();
            for p in up {
                st.due.insert(p, Instant::now() + FIRST_CHECK);
            }
            d.cv.notify_all();
        }
        let busy = {
            let st = d.lock();
            !st.m
                .snap
                .profiles
                .iter()
                .all(|p| p.session.is_none() && p.busy.is_none())
                || !st.m.snap.others.is_empty()
                || !st.starting.is_empty()
        };
        if busy {
            rescan(&d);
        } else {
            let mut st = d.lock();
            refresh(&d, &mut st);
        }
    }
}

/// Runs one profile's checks: its probes in parallel, then its DNS. Call
/// it on a helper thread.
fn run_checks(d: &Shared, profile: &str) -> Option<Report> {
    let (p, device, timeout) = {
        let mut st = d.lock();
        let p = st.m.cfg.profile(profile)?.clone();
        let device =
            st.m.snap
                .profile(profile)
                .and_then(|v| v.session.as_ref())
                .map(|s| s.device.clone())
                .unwrap_or_default();
        let timeout = Duration::from_secs(st.m.cfg.checks.timeout.max(1));
        st.m.checking(profile, true);
        refresh(d, &mut st);
        (p, device, timeout)
    };
    let handles: Vec<_> = p
        .probes
        .iter()
        .cloned()
        .map(|t| std::thread::spawn(move || checks::probe(&t, timeout)))
        .collect();
    let probes = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    let mut r = Report {
        at: now(),
        probes,
        ..Default::default()
    };
    if !device.is_empty() {
        r.dns = dbus::link_dns(&d.conn, &device);
        if let Some(dns) = &r.dns {
            r.dns_missing = dns.missing(&p.dns, &p.zones);
            if p.apply_dns && !r.dns_missing.is_empty() {
                match dbus::set_link_dns(&d.conn, &device, &p.dns, &p.zones) {
                    Ok(()) => {
                        r.dns_applied = true;
                        r.dns = dbus::link_dns(&d.conn, &device);
                        r.dns_missing = r
                            .dns
                            .as_ref()
                            .map(|x| x.missing(&p.dns, &p.zones))
                            .unwrap_or_default();
                    }
                    Err(e) => r.dns_error = Some(format!("{e:#}")),
                }
            }
        }
    }
    let mut st = d.lock();
    st.m.checked(profile, r.clone());
    refresh(d, &mut st);
    Some(r)
}

/// Runs checks when they are due: a few seconds after a tunnel comes up,
/// every `checks.every` seconds while it is up, and sooner after one fails.
fn checker(d: Shared) {
    loop {
        let todo: Vec<String> = {
            let mut st = d.lock();
            loop {
                let now_i = Instant::now();
                let due: Vec<String> = st
                    .due
                    .iter()
                    .filter(|(_, t)| **t <= now_i)
                    .map(|(p, _)| p.clone())
                    .collect();
                if !due.is_empty() {
                    for p in &due {
                        st.due.remove(p);
                    }
                    break due;
                }
                let wait = st
                    .due
                    .values()
                    .map(|t| t.saturating_duration_since(now_i))
                    .min()
                    .unwrap_or(Duration::from_secs(3600));
                st =
                    d.cv.wait_timeout(st, wait)
                        .map(|(g, _)| g)
                        .unwrap_or_else(|e| e.into_inner().0);
            }
        };
        for p in todo {
            let up = d.lock().m.phase(&p).up();
            if !up {
                continue;
            }
            run_checks(&d, &p);
            // Next one.
            let mut st = d.lock();
            if st.m.phase(&p).up() {
                let every = st.m.cfg.checks.every;
                let next = if st.m.fails(&p) > 0 {
                    Some(RECHECK)
                } else if every > 0 {
                    Some(Duration::from_secs(every))
                } else {
                    None
                };
                if let Some(n) = next {
                    let at = Instant::now() + n;
                    let e = st.due.entry(p).or_insert(at);
                    if *e > at {
                        *e = at;
                    }
                }
            }
        }
    }
}

/// Runs the doctor and records what it found.
fn examine(d: &Shared) {
    let cfg = d.lock().m.cfg.doctor.clone();
    let found = doctor::examine(&cfg, now());
    let mut st = d.lock();
    st.m.set_doctor(found, now());
    refresh(d, &mut st);
}

/// Runs the doctor at start and whenever the dynamic linker's cache
/// changes (a package update).
fn doctor_watch(d: Shared) {
    let mut seen = None;
    loop {
        let stamp = doctor::ld_cache_stamp();
        if seen.is_none() || stamp != seen.flatten() {
            seen = Some(stamp);
            examine(&d);
        }
        std::thread::sleep(DOCTOR_EVERY);
    }
}

/// Splits a configured command line into words (no shell).
fn words(cmd: &str) -> Vec<String> {
    tablet_core::config::expand_command(cmd)
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// Starts `openvpn3 session-start --config <ovpn>` for a profile on a
/// helper thread. openvpn3 opens the web sign-in page itself.
fn start(d: &Shared, profile: &str) -> Result<()> {
    let (cmd, ovpn, timeout) = {
        let st = d.lock();
        let p =
            st.m.cfg
                .profile(profile)
                .ok_or_else(|| anyhow!("there is no profile `{profile}`"))?;
        let ovpn = p.ovpn_path();
        if !ovpn.exists() {
            return Err(anyhow!("{} does not exist", paths::tilde(&ovpn)));
        }
        (
            words(&st.m.cfg.openvpn3.command),
            ovpn,
            Duration::from_secs(st.m.cfg.openvpn3.start_timeout.max(10)),
        )
    };
    let Some((program, args)) = cmd.split_first() else {
        return Err(anyhow!("openvpn3.command is empty"));
    };
    let mut c = std::process::Command::new(program);
    c.args(args)
        .arg("session-start")
        .arg("--config")
        .arg(&ovpn)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    {
        let mut st = d.lock();
        st.m.connecting(profile, now());
        st.starting.push(profile.to_string());
        refresh(d, &mut st);
    }
    let mut child = match c.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut st = d.lock();
            st.starting.retain(|p| p != profile);
            st.m.start_failed(profile, format!("cannot run {program}: {e}"));
            refresh(d, &mut st);
            return Err(anyhow!("cannot run {program}: {e}"));
        }
    };
    eprintln!(
        "tablet: {profile}: {program} session-start --config {}",
        ovpn.display()
    );
    let last = Arc::new(Mutex::new(String::new()));
    for out in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let last = last.clone();
        let name = profile.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                // The sign-in URL carries a one-time token: never log it.
                let shown = match tablet_core::openvpn::find_url(&line) {
                    Some(u) => line.replace(&u, "<sign-in page>"),
                    None => line.clone(),
                };
                eprintln!("tablet: {name}: openvpn3: {shown}");
                let t = shown
                    .trim()
                    .trim_start_matches("** ERROR **")
                    .trim()
                    .to_string();
                if !t.is_empty() {
                    *last.lock().unwrap_or_else(|e| e.into_inner()) = t;
                }
            }
        });
    }
    let d2 = d.clone();
    let profile = profile.to_string();
    std::thread::spawn(move || {
        let started = Instant::now();
        // Look for the new session soon; signals may be late.
        let d3 = d2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            rescan(&d3);
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if started.elapsed() > timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(250)),
                Err(_) => break None,
            }
        };
        // Let the output threads finish their last line.
        std::thread::sleep(Duration::from_millis(100));
        let said = last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        rescan(&d2);
        let mut st = d2.lock();
        st.starting.retain(|p| p != &profile);
        match status {
            Some(s) if s.success() => st.m.start_done(&profile),
            Some(s) => {
                // A session that exists (the sign-in still waiting, say)
                // speaks for itself; only a start with nothing to show for
                // it is a failure here.
                if st.m.path_of(&profile).is_none() {
                    let why = if said.is_empty() {
                        format!("openvpn3 session-start exited with {s}")
                    } else {
                        said
                    };
                    st.m.start_failed(&profile, why);
                } else {
                    st.m.start_done(&profile);
                }
            }
            None => {
                if st.m.path_of(&profile).is_none() {
                    st.m.start_failed(
                        &profile,
                        "openvpn3 session-start did not finish in time".into(),
                    );
                }
            }
        }
        refresh(&d2, &mut st);
    });
    Ok(())
}

/// The profile a request names, or the default one.
fn pick(st: &State, profile: Option<String>) -> Result<String> {
    match profile {
        Some(p) if st.m.cfg.profile(&p).is_some() => Ok(p),
        Some(p) => Err(anyhow!(
            "there is no profile `{p}` in {}",
            paths::tilde(&paths::config_file())
        )),
        None => {
            st.m.cfg
                .default_profile()
                .map(|p| p.name.clone())
                .ok_or_else(|| {
                    anyhow!(
                        "no profiles yet: add a [[profile]] to {}",
                        paths::tilde(&paths::config_file())
                    )
                })
        }
    }
}

fn connect(d: &Shared, profile: Option<String>) -> Result<String> {
    let name = pick(&d.lock(), profile)?;
    // A broken openvpn3 gets a fresh look first: it may have been rebuilt.
    if d.lock().m.doctor().broken() {
        examine(d);
        let st = d.lock();
        if st.m.doctor().broken() {
            return Err(anyhow!("{}; see `tablet doctor`", st.m.doctor().summary()));
        }
    }
    let phase = d.lock().m.phase(&name);
    match phase {
        Phase::Held => return Ok(format!("{name} is already connected")),
        Phase::Starting | Phase::SignIn | Phase::Reconnecting => {
            return Ok(format!("{name} is already connecting"));
        }
        Phase::Stale | Phase::Failed | Phase::Dropped => return reconnect(d, Some(name)),
        Phase::Off => {}
    }
    start(d, &name)?;
    Ok(format!("connecting {name}"))
}

/// Ends a profile's sessions (or every session when `profile` is None).
fn disconnect(d: &Shared, profile: Option<String>) -> Result<String> {
    let (paths, names) = {
        let mut st = d.lock();
        match profile {
            Some(p) => {
                let name = pick(&st, Some(p))?;
                st.m.stopping(&name);
                (st.m.paths_of(&name), vec![name])
            }
            None => {
                let names: Vec<String> = st.m.cfg.profiles.iter().map(|p| p.name.clone()).collect();
                for n in &names {
                    st.m.stopping(n);
                }
                let mut paths: Vec<String> = names.iter().flat_map(|n| st.m.paths_of(n)).collect();
                paths.extend(st.m.snap.others.iter().map(|o| o.path.clone()));
                (paths, names)
            }
        }
    };
    {
        let mut st = d.lock();
        refresh(d, &mut st);
    }
    let mut errors = vec![];
    for p in &paths {
        if let Err(e) = dbus::session_call(&d.conn, p, "Disconnect") {
            errors.push(format!("{e:#}"));
        }
    }
    rescan(d);
    if !errors.is_empty() {
        return Err(anyhow!(errors.join("; ")));
    }
    Ok(if paths.is_empty() {
        format!("{} had no session", names.join(", "))
    } else {
        format!("disconnected {}", names.join(", "))
    })
}

fn reconnect(d: &Shared, profile: Option<String>) -> Result<String> {
    let name = pick(&d.lock(), profile)?;
    let paths = {
        let mut st = d.lock();
        st.m.stopping(&name);
        st.m.paths_of(&name)
    };
    for p in &paths {
        let _ = dbus::session_call(&d.conn, p, "Disconnect");
    }
    if !paths.is_empty() {
        // Give the old tunnel a moment to go, so the new one gets its
        // routes.
        std::thread::sleep(Duration::from_millis(600));
    }
    start(d, &name)?;
    Ok(format!("reconnecting {name}"))
}

fn ack(d: &Shared, profile: Option<String>) -> Result<String> {
    let names: Vec<String> = {
        let st = d.lock();
        match profile {
            Some(p) => vec![pick(&st, Some(p))?],
            None => {
                st.m.snap
                    .profiles
                    .iter()
                    .filter(|p| matches!(p.phase, Phase::Failed | Phase::Dropped))
                    .map(|p| p.name.clone())
                    .collect()
            }
        }
    };
    for n in &names {
        let (paths, failed) = {
            let mut st = d.lock();
            let failed = st.m.phase(n) == Phase::Failed;
            st.m.ack(n);
            refresh(d, &mut st);
            (st.m.paths_of(n), failed)
        };
        // A failed session that lingers is cleared away.
        if failed {
            for p in &paths {
                let _ = dbus::session_call(&d.conn, p, "Disconnect");
            }
        }
    }
    rescan(d);
    Ok(if names.is_empty() {
        "nothing to dismiss".into()
    } else {
        format!("dismissed {}", names.join(", "))
    })
}

/// Runs a command detached, with its URL or command as the last argument.
fn open_with(opener: &str, arg: &str) -> Result<()> {
    let w = words(opener);
    let (program, args) = w.split_first().ok_or_else(|| anyhow!("no opener set"))?;
    let mut c = std::process::Command::new(program);
    c.args(args)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = c
        .spawn()
        .map_err(|e| anyhow!("cannot run {program}: {e}"))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn open_signin(d: &Shared, profile: Option<String>) -> Result<String> {
    let (url, opener, name) = {
        let st = d.lock();
        let waiting =
            st.m.snap
                .profiles
                .iter()
                .find(|p| p.signin_url.is_some() && profile.as_deref().is_none_or(|n| n == p.name));
        let Some(p) = waiting else {
            return Err(anyhow!("no sign-in page is waiting"));
        };
        (
            p.signin_url.clone().unwrap_or_default(),
            st.m.cfg.openvpn3.opener.clone(),
            p.name.clone(),
        )
    };
    open_with(&opener, &url)?;
    Ok(format!("opened {name}'s sign-in page"))
}

/// The lamp's click.
fn flip(d: &Shared) -> Result<String> {
    let (ask, signin, default, phase, others) = {
        let st = d.lock();
        let s = &st.m.snap;
        let default = st.m.cfg.default_profile().map(|p| p.name.clone());
        let phase = default.as_deref().map(|n| st.m.phase(n));
        (
            s.asks.first().map(|a| a.id),
            s.profiles
                .iter()
                .find(|p| p.phase == Phase::SignIn)
                .map(|p| p.name.clone()),
            default,
            phase,
            !s.others.is_empty(),
        )
    };
    if let Some(id) = ask {
        return answer(d, id, true);
    }
    if let Some(p) = signin {
        return open_signin(d, Some(p));
    }
    match (default, phase) {
        (Some(n), Some(Phase::Off)) => connect(d, Some(n)),
        (Some(n), Some(Phase::Held | Phase::Starting | Phase::Reconnecting)) => {
            disconnect(d, Some(n))
        }
        (Some(n), Some(_)) => reconnect(d, Some(n)),
        _ if others => disconnect(d, None),
        _ => Err(anyhow!(
            "no profiles yet: add a [[profile]] to {}",
            paths::tilde(&paths::config_file())
        )),
    }
}

fn answer(d: &Shared, id: u64, yes: bool) -> Result<String> {
    if yes {
        let profile = d.lock().m.answer(id, true);
        let Some(p) = profile else {
            return Err(anyhow!("that request is no longer waiting"));
        };
        let said = connect(d, Some(p))?;
        let mut st = d.lock();
        refresh(d, &mut st);
        Ok(said)
    } else {
        let mut st = d.lock();
        st.m.drop_ask(id);
        st.resolved.insert(id, Err("you said Not now".into()));
        d.cv.notify_all();
        refresh(d, &mut st);
        Ok("declined".into())
    }
}

/// An agent's request: waits until it settles or times out.
fn ask(d: &Shared, a: Ask, timeout: Duration) -> Reply {
    let profile = a.profile.clone();
    let id = {
        let mut st = d.lock();
        if st.m.cfg.profile(&profile).is_none() {
            return Reply::err(format!("there is no profile `{profile}`"));
        }
        if st.m.phase(&profile) == Phase::Held {
            return Reply::said(format!("{profile} is connected"));
        }
        let id = st.m.ask(a);
        refresh(d, &mut st);
        id
    };
    let deadline = Instant::now() + timeout;
    let mut st = d.lock();
    loop {
        if let Some(r) = st.resolved.remove(&id) {
            return match r {
                Ok(()) => Reply::said(format!("{profile} is connected")),
                Err(e) => Reply::err(e),
            };
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            st.m.drop_ask(id);
            refresh(d, &mut st);
            return Reply::err(format!(
                "nobody answered in {}",
                tablet_core::text::span(timeout.as_secs_f64())
            ));
        }
        st =
            d.cv.wait_timeout(st, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
    }
}

/// Runs `doctor.repair`, in `doctor.terminal` when one is set, then looks
/// again.
fn repair(d: &Shared) -> Result<String> {
    let cfg = d.lock().m.cfg.doctor.clone();
    let cmd = cfg.repair.trim().to_string();
    if cmd.is_empty() {
        return Err(anyhow!(
            "doctor.repair is not set; rebuild or reinstall openvpn3 by hand"
        ));
    }
    let mut c = if cfg.terminal.trim().is_empty() {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(&cmd);
        c
    } else {
        let w = words(&cfg.terminal);
        let (program, args) = w
            .split_first()
            .ok_or_else(|| anyhow!("doctor.terminal is empty"))?;
        let mut c = std::process::Command::new(program);
        c.args(args).arg(&cmd);
        c
    };
    c.stdin(std::process::Stdio::null());
    if let Ok(f) = paths::open_log(&paths::daemon_log())
        && let Ok(f2) = f.try_clone()
    {
        c.stdout(f).stderr(f2);
    }
    let mut child = c
        .spawn()
        .map_err(|e| anyhow!("cannot run the repair: {e}"))?;
    eprintln!("tablet: repairing openvpn3: {cmd}");
    let d = d.clone();
    std::thread::spawn(move || {
        let s = child.wait();
        eprintln!("tablet: repair finished: {s:?}");
        examine(&d);
    });
    Ok(format!("running {cmd}"))
}

/// Starts `tablet-board`, preferring the one installed next to `tablet`.
fn spawn_board() {
    let local = paths::exe()
        .as_ref()
        .and_then(|e| e.parent())
        .map(|dir| dir.join("tablet-board"))
        .filter(|p| p.exists());
    let bin = local
        .map(|p| p.into_os_string())
        .unwrap_or_else(|| "tablet-board".into());
    let mut cmd = std::process::Command::new(&bin);
    cmd.stdin(std::process::Stdio::null());
    if let Ok(f) = paths::open_log(&paths::board_log())
        && let Ok(f2) = f.try_clone()
    {
        cmd.stdout(f).stderr(f2);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("tablet: cannot start {}: {e}", bin.to_string_lossy()),
    }
}

fn to_board(st: &mut State, ev: Event) {
    if broadcast(&mut st.boards, &event_line(&ev)) > 0 {
        return;
    }
    let show = match ev {
        Event::Toggle { anchor } | Event::Show { anchor } => Event::Show { anchor },
        other => other,
    };
    if matches!(show, Event::Hide) {
        return;
    }
    st.pending = Some(show);
    if st
        .board_started
        .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
    {
        st.board_started = Some(Instant::now());
        spawn_board();
    }
}

fn snapshot_reply(d: &Shared) -> Reply {
    Reply {
        snapshot: Some(Box::new(d.lock().m.snap.clone())),
        ..Reply::ok()
    }
}

fn result(r: Result<String>) -> Reply {
    match r {
        Ok(m) => Reply::said(m),
        Err(e) => Reply::err(format!("{e:#}")),
    }
}

fn handle(d: Shared, stream: Stream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    let reader = BufReader::new(read_half);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_line(
                    &mut writer,
                    &serde_json::to_string(&Reply::err(format!("bad request: {e}")))
                        .unwrap_or_default(),
                );
                continue;
            }
        };
        let reply = match req {
            Request::Ping => Reply {
                version: Some(env!("CARGO_PKG_VERSION").into()),
                stale: paths::exe_replaced(),
                ..Reply::ok()
            },
            Request::Quit => {
                let _ = send_line(
                    &mut writer,
                    &serde_json::to_string(&Reply::ok()).unwrap_or_default(),
                );
                retire(&mut d.lock());
            }
            Request::Snapshot => snapshot_reply(&d),
            Request::Subscribe { kind } => {
                let _ = writer.set_write_timeout(Some(Duration::from_secs(2)));
                let mut st = d.lock();
                match kind {
                    SubKind::Board => {
                        let sub = Sub::new(writer);
                        let mut ok = sub.send(&event_line(&Event::Snapshot {
                            snapshot: Box::new(st.m.snap.clone()),
                        }));
                        if let Some(p) = st.pending.take() {
                            ok &= sub.send(&event_line(&p));
                        }
                        if ok {
                            st.boards.push(sub);
                        }
                    }
                    SubKind::Bar => {
                        let sub = Sub::new(writer);
                        let line = st.m.bar_line().to_string();
                        if line.is_empty() || sub.send(&line) {
                            st.bars.push(sub);
                        }
                    }
                }
                return;
            }
            Request::Toggle { anchor } => {
                to_board(&mut d.lock(), Event::Toggle { anchor });
                Reply::ok()
            }
            Request::Show { anchor } => {
                to_board(&mut d.lock(), Event::Show { anchor });
                Reply::ok()
            }
            Request::Hide => {
                let mut st = d.lock();
                st.pending = None;
                broadcast(&mut st.boards, &event_line(&Event::Hide));
                Reply::ok()
            }
            Request::BoardVisible { .. } => Reply::ok(),
            Request::Connect { profile } => result(connect(&d, profile)),
            Request::Disconnect { profile } => result(disconnect(&d, profile)),
            Request::Reconnect { profile } => result(reconnect(&d, profile)),
            Request::Flip => result(flip(&d)),
            Request::Ack { profile } => result(ack(&d, profile)),
            Request::OpenSignIn { profile } => result(open_signin(&d, profile)),
            Request::Check { profile } => {
                rescan(&d);
                let names: Vec<String> = {
                    let st = d.lock();
                    match &profile {
                        Some(p) => vec![p.clone()],
                        None => {
                            st.m.snap
                                .profiles
                                .iter()
                                .filter(|p| p.phase.up())
                                .map(|p| p.name.clone())
                                .collect()
                        }
                    }
                };
                let handles: Vec<_> = names
                    .into_iter()
                    .map(|n| {
                        let d = d.clone();
                        std::thread::spawn(move || run_checks(&d, &n))
                    })
                    .collect();
                for h in handles {
                    let _ = h.join();
                }
                snapshot_reply(&d)
            }
            Request::Ask {
                profile,
                reason,
                command,
                cwd,
                requester,
                timeout,
            } => ask(
                &d,
                Ask {
                    profile,
                    reason,
                    command,
                    cwd,
                    requester,
                    at: now(),
                    ..Default::default()
                },
                Duration::from_secs(timeout.clamp(1, 3600)),
            ),
            Request::Answer { id, connect } => result(answer(&d, id, connect)),
            Request::Doctor => {
                examine(&d);
                snapshot_reply(&d)
            }
            Request::Repair => result(repair(&d)),
        };
        if !send_line(
            &mut writer,
            &serde_json::to_string(&reply).unwrap_or_default(),
        ) {
            return;
        }
    }
}

fn reload_config(d: &Shared) {
    let (cfg, err) = Config::load();
    let mut st = d.lock();
    if !st.m.reload(cfg, err) {
        return;
    }
    refresh(d, &mut st);
    drop(st);
    let d = d.clone();
    std::thread::spawn(move || rescan(&d));
}

pub fn run() -> Result<i32> {
    let listener = match Listener::bind() {
        Ok(l) => l,
        Err(e) if e.is::<tablet_core::transport::AlreadyRunning>() => {
            eprintln!("tablet: {e}");
            return Ok(tablet_core::transport::ALREADY_RUNNING);
        }
        Err(e) => return Err(e),
    };
    let conn = dbus::connect()?;
    let (cfg, cfg_error) = Config::load();
    if let Some(e) = &cfg_error {
        eprintln!("tablet: {e}");
    }
    let d: Shared = Arc::new(Daemon {
        state: Mutex::new(State {
            m: Model::new(cfg, cfg_error, load_history()),
            boards: vec![],
            bars: vec![],
            pending: None,
            board_started: None,
            due: HashMap::new(),
            resolved: HashMap::new(),
            starting: vec![],
        }),
        cv: Condvar::new(),
        conn,
    });
    // Watch first, then scan, so nothing that changes in between is lost.
    keep(&d, "session manager", watch_manager);
    keep(&d, "status", watch_status);
    keep(&d, "properties", watch_props);
    keep(&d, "names", watch_names);
    rescan(&d);
    for (name, f) in [
        ("watcher", watcher as fn(Shared)),
        ("checker", checker),
        ("doctor", doctor_watch),
    ] {
        let d = d.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || f(d))?;
    }
    {
        let d = d.clone();
        std::thread::spawn(move || {
            crate::watch::watch_file(&paths::config_file(), || reload_config(&d));
            eprintln!("tablet: cannot watch the config; restart to pick up changes");
        });
    }
    eprintln!("tablet: listening on {}", paths::socket_path().display());
    loop {
        match listener.accept() {
            Ok(stream) => {
                let d = d.clone();
                std::thread::spawn(move || handle(d, stream));
            }
            Err(e) => {
                eprintln!("tablet: accept failed: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Stream, Stream) {
        let (a, b) = Stream::pair().unwrap();
        a.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        (a, b)
    }

    #[test]
    fn subscribers_get_lines_in_order() {
        let (a, b) = pair();
        let mut subs = vec![Sub::new(a)];
        assert_eq!(broadcast(&mut subs, "one"), 1);
        assert_eq!(broadcast(&mut subs, "two"), 1);
        subs.pop().unwrap().finish();
        let lines: Vec<String> = BufReader::new(b).lines().map(|l| l.unwrap()).collect();
        assert_eq!(lines, vec!["one", "two"]);
    }

    #[test]
    fn a_stuck_subscriber_never_blocks_a_broadcast() {
        let (a, unread) = pair();
        let mut subs = vec![Sub::new(a)];
        let big = "x".repeat(64 * 1024);
        let start = Instant::now();
        for _ in 0..(SUB_QUEUE + 40) {
            broadcast(&mut subs, &big);
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(subs.is_empty());
        drop(unread);
    }

    #[test]
    fn command_words() {
        assert_eq!(words("openvpn3"), vec!["openvpn3"]);
        assert_eq!(
            words(" kitty --hold sh -c "),
            vec!["kitty", "--hold", "sh", "-c"]
        );
        assert!(words("~/bin/x").first().unwrap().ends_with("/bin/x"));
    }
}
