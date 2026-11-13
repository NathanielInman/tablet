//! Command-line entry points.

use anyhow::{Result, anyhow};
use std::io::Write;
use std::time::Duration;
use tablet_core::config::Config;
use tablet_core::model::{Phase, ProfileView, Snapshot};
use tablet_core::protocol::{Request, SubKind};
use tablet_core::route::{self, Route};
use tablet_core::text::{ago, bytes, span, when};
use tablet_core::{now, paths};

/// The exit status for "the VPN is the problem": a profile is down, stale
/// or declined (EX_TEMPFAIL, as `tablet daemon` uses for "already
/// running").
pub const VPN_DOWN: i32 = 75;

const HELP: &str = "\
tablet: the VPN lamp for waybar, around openvpn3

Usage:
  tablet connect [profile]          start a session (no profile: the default one)
  tablet disconnect [profile|--all]  end a profile's session, or every session
  tablet reconnect [profile]        end the session and start a new one
  tablet flip                       what a click on the lamp does
  tablet ack [profile]              dismiss a dropped or failed profile
  tablet signin [profile]           open the waiting sign-in page again
  tablet check [profile]            check now: probes, DNS and traffic (exit 75
                                    when a tunnel does not work)
  tablet why <host|url>             which profile a host needs and whether it
                                    is up (exit 75 when it is down)
  tablet need [profile] [--reason <text>] [--timeout <secs>] -- <command>...
                                    run a command once the profile is
                                    connected, asking you on the bar first
                                    (exit 75 when it is not)
  tablet status [--json]            every profile and session
  tablet history [--count <n>]      what happened, newest first
  tablet doctor [--repair]          check openvpn3, the config and the bar
  tablet toggle [--center]          open or close the drawer from the clicked icon
  tablet show [--center] | hide     open or close the drawer
  tablet bar                        the waybar module (prints JSON lines)
  tablet daemon                     run the daemon in the foreground
  tablet restart                    restart the daemon and the drawer
  tablet --version

Config: ~/.config/tablet/config.toml
";

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// The first argument that is not a flag.
fn positional(args: &[String]) -> Option<String> {
    args.iter().find(|a| !a.starts_with('-')).cloned()
}

pub fn run(args: &[String]) -> Result<i32> {
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{HELP}");
            Ok(0)
        }
        Some("-V" | "--version" | "version") => {
            println!("tablet {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some("daemon") => crate::daemon::run(),
        Some("restart") => restart(),
        Some("bar") => bar(),
        Some(cmd @ ("toggle" | "show")) => open(cmd == "toggle", rest),
        Some("hide") => {
            if tablet_core::transport::daemon_alive() {
                send(&Request::Hide)?;
            }
            Ok(0)
        }
        Some("connect" | "up") => command(Request::Connect {
            profile: positional(rest),
        }),
        Some("disconnect" | "down") => command(Request::Disconnect {
            profile: if flag(rest, "--all") {
                None
            } else {
                Some(positional(rest).map_or_else(default_name, Ok)?)
            },
        }),
        Some("reconnect") => command(Request::Reconnect {
            profile: positional(rest),
        }),
        Some("flip") => command(Request::Flip),
        Some("ack" | "dismiss") => command(Request::Ack {
            profile: positional(rest),
        }),
        Some("signin" | "sign-in") => command(Request::OpenSignIn {
            profile: positional(rest),
        }),
        Some("check") => check(positional(rest)),
        Some("why") => why(rest),
        Some("need") => need(rest),
        Some("status") => status(flag(rest, "--json")),
        Some("history") => history(rest),
        Some("doctor") => doctor(flag(rest, "--repair")),
        Some(other) => Err(anyhow!("unknown command `{other}` (try `tablet --help`)")),
    }
}

fn default_name() -> Result<String> {
    let (cfg, _) = Config::load();
    cfg.default_profile()
        .map(|p| p.name.clone())
        .ok_or_else(|| {
            anyhow!(
                "no profiles yet: add a [[profile]] to {}",
                paths::tilde(&paths::config_file())
            )
        })
}

fn send(req: &Request) -> Result<Option<String>> {
    let r = tablet_core::protocol::request(req)?;
    match r.error {
        Some(e) => Err(anyhow!(e)),
        None => Ok(r.message),
    }
}

/// Bar clicks and commands: start the daemon if needed, then ask it.
fn command(req: Request) -> Result<i32> {
    ensure_daemon()?;
    if let Some(m) = send(&req)? {
        println!("{m}");
    }
    Ok(0)
}

/// Asks the daemon to quit and waits until that daemon is gone.
fn stop_daemon() -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(mut s) =
        tablet_core::transport::connect_to(&paths::socket_path(), Some(Duration::from_secs(2)))
    else {
        return true;
    };
    let line = serde_json::to_string(&Request::Quit).unwrap_or_default();
    if writeln!(s, "{line}").is_err() {
        return true;
    }
    let mut lines = BufReader::new(s).lines();
    let ok = match lines.next() {
        Some(Ok(reply)) => {
            serde_json::from_str::<tablet_core::protocol::Reply>(&reply).is_ok_and(|r| r.ok)
        }
        _ => true,
    };
    ok && lines.all(|l| l.is_ok())
}

fn restart() -> Result<i32> {
    if tablet_core::transport::daemon_alive() && !stop_daemon() {
        return Err(anyhow!(
            "the running daemon did not quit; stop it with `pkill -f 'tablet daemon'`"
        ));
    }
    start_daemon()?;
    println!("tablet: daemon restarted; the drawer starts on the next click");
    Ok(0)
}

/// Starts the daemon in the background when it is not running. A daemon
/// whose binary an upgrade replaced is swapped for the new one first.
fn ensure_daemon() -> Result<()> {
    if tablet_core::transport::daemon_alive() {
        let stale = tablet_core::protocol::request_within(&Request::Ping, Duration::from_secs(1))
            .is_ok_and(|r| r.stale);
        if !stale || !stop_daemon() {
            return Ok(());
        }
    }
    start_daemon()
}

fn daemon_answers() -> bool {
    tablet_core::protocol::request_within(&Request::Ping, Duration::from_millis(500)).is_ok()
}

/// Starts a daemon in the background and waits until one answers. A new
/// daemon gives up when another holds the lock; one that gave up while
/// nobody answers is started again (the old one was just leaving).
fn start_daemon() -> Result<()> {
    let exe = paths::exe().ok_or_else(|| anyhow!("cannot find the tablet binary"))?;
    let log = paths::daemon_log();
    std::fs::create_dir_all(paths::state_dir())?;
    let spawn = || -> Result<std::process::Child> {
        let out = paths::open_log(&log)?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out);
        use std::os::unix::process::CommandExt;
        // Its own session, so restarting the bar does not take the daemon
        // with it.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        Ok(cmd.spawn()?)
    };
    let mut child = spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        if daemon_answers() {
            std::thread::spawn(move || child.wait());
            return Ok(());
        }
        if let Ok(Some(_)) = child.try_wait() {
            child = spawn()?;
        }
    }
    Err(anyhow!("the daemon did not start; see {}", log.display()))
}

fn open(toggle: bool, args: &[String]) -> Result<i32> {
    let (cfg, _) = Config::load();
    // Ask the compositor first: the cursor is still on the icon.
    let hint = tablet_core::waybar::load_layout()
        .unwrap_or_default()
        .hint();
    let anchor = bar_anchor::anchor(&cfg.bar.namespace, hint, flag(args, "--center"));
    ensure_daemon()?;
    send(&if toggle {
        Request::Toggle { anchor }
    } else {
        Request::Show { anchor }
    })?;
    Ok(0)
}

/// Runs the new `tablet bar` in this process after an upgrade replaced the
/// binary.
fn reexec_bar() {
    use std::os::unix::process::CommandExt;
    if let Some(exe) = paths::exe() {
        let err = std::process::Command::new(exe).arg("bar").exec();
        eprintln!("tablet: cannot restart the bar module: {err}");
    }
}

fn bar() -> Result<i32> {
    let (cfg, _) = Config::load();
    let offline = serde_json::to_string(&tablet_core::bar::offline(&cfg))?;
    let mut out = std::io::stdout().lock();
    let mut last = String::new();
    let mut started: Option<std::time::Instant> = None;
    loop {
        if paths::exe_replaced() {
            reexec_bar();
        }
        match tablet_core::protocol::subscribe_raw(SubKind::Bar) {
            Ok(lines) => {
                for line in lines {
                    let Ok(line) = line else { break };
                    if line != last {
                        writeln!(out, "{line}")?;
                        out.flush()?;
                        last = line;
                    }
                }
                if paths::exe_replaced() {
                    continue;
                }
            }
            Err(_) if started.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) => {
                started = Some(std::time::Instant::now());
                let _ = ensure_daemon();
                continue;
            }
            Err(_) => {}
        }
        if last != offline {
            writeln!(out, "{offline}")?;
            out.flush()?;
            last = offline.clone();
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn snapshot() -> Option<Snapshot> {
    tablet_core::protocol::request(&Request::Snapshot)
        .ok()?
        .snapshot
        .map(|s| *s)
}

/// One profile in a line: `work  held 2h 14m · tun0 · vpn.example.com`.
fn profile_line(p: &ProfileView, t: f64) -> String {
    let mut s = format!("{:<10}{}", p.name, p.phase.label());
    match p.phase {
        Phase::Held | Phase::Stale | Phase::Reconnecting => {
            if let Some(at) = p.connected_at {
                s.push_str(&format!(" {}", span(t - at)));
            }
        }
        Phase::Off => {}
        _ => s.push_str(&format!(" since {}", when(p.since, t))),
    }
    if let Some(sess) = &p.session {
        if !sess.device.is_empty() {
            s.push_str(&format!(" \u{b7} {}", sess.device));
        }
        if !sess.server.is_empty() {
            s.push_str(&format!(" \u{b7} {}", sess.server));
        }
    }
    if p.default {
        s.push_str(" (default)");
    }
    s
}

fn status(json: bool) -> Result<i32> {
    ensure_daemon()?;
    let Some(s) = snapshot() else {
        println!(
            "{}",
            if json {
                "null"
            } else {
                "The Tablet daemon did not answer."
            }
        );
        return Ok(1);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(0);
    }
    let t = now();
    if s.profiles.is_empty() {
        println!(
            "No profiles yet: add a [[profile]] to {}.",
            paths::tilde(&paths::config_file())
        );
    }
    for p in &s.profiles {
        println!("{}", profile_line(p, t));
        if let Some(r) = &p.reason
            && p.phase != Phase::Held
        {
            println!("          {r}");
        }
        if let Some(c) = &p.checks {
            println!(
                "          checked {}: {}/{} probes answered{}",
                ago(t - c.at),
                c.probes.len() - c.failed(),
                c.probes.len(),
                if c.dns_missing.is_empty() {
                    String::new()
                } else {
                    format!(", DNS missing {}", c.dns_missing.join(", "))
                }
            );
        }
        if p.signin_url.is_some() {
            println!(
                "          a sign-in page is waiting: `tablet signin {}`",
                p.name
            );
        }
    }
    for o in &s.others {
        println!(
            "{:<10}{} \u{b7} {} (no profile)",
            "-",
            o.status.describe(),
            o.config_name
        );
    }
    for a in &s.asks {
        println!(
            "asking    {} wants {}: {}",
            if a.requester.is_empty() {
                "an agent"
            } else {
                &a.requester
            },
            a.profile,
            a.reason
        );
    }
    println!("{:<10}{}", "openvpn3", s.doctor.summary());
    if let Some(e) = &s.config_error {
        println!("\nconfig: {e}");
    }
    Ok(0)
}

fn check(profile: Option<String>) -> Result<i32> {
    ensure_daemon()?;
    let r = tablet_core::protocol::request(&Request::Check {
        profile: profile.clone(),
    })?;
    if let Some(e) = r.error {
        return Err(anyhow!(e));
    }
    let Some(s) = r.snapshot else { return Ok(1) };
    let t = now();
    let mut bad = false;
    let shown: Vec<&ProfileView> = s
        .profiles
        .iter()
        .filter(|p| match &profile {
            Some(n) => &p.name == n,
            None => p.phase != Phase::Off,
        })
        .collect();
    if shown.is_empty() {
        println!("Nothing is connected.");
        return Ok(if profile.is_some() { VPN_DOWN } else { 0 });
    }
    for p in shown {
        println!("{}", profile_line(p, t));
        if p.phase != Phase::Held {
            bad = true;
            if let Some(r) = &p.reason {
                println!("  {:<6}{r}", "why");
            }
        }
        if let Some(c) = &p.checks {
            let width = c.probes.iter().map(|p| p.target.len()).max().unwrap_or(0) + 2;
            for pr in &c.probes {
                bad |= !pr.ok;
                println!(
                    "  {:<6}{:<width$}{}",
                    if pr.ok { "ok" } else { "fail" },
                    pr.target,
                    if pr.ok {
                        format!("{} ms", pr.ms)
                    } else {
                        pr.error.clone().unwrap_or_default()
                    }
                );
            }
            match (&c.dns, c.dns_missing.is_empty()) {
                (Some(d), true) => println!(
                    "  {:<6}DNS: {} \u{b7} {}",
                    "ok",
                    if d.servers.is_empty() {
                        "no resolvers".into()
                    } else {
                        d.servers.join(", ")
                    },
                    if d.domains.is_empty() {
                        "no domains".into()
                    } else {
                        d.domains.join(" ")
                    }
                ),
                (Some(_), false) => {
                    bad = true;
                    println!("  {:<6}DNS is missing {}", "fail", c.dns_missing.join(", "));
                }
                (None, _) => {}
            }
            if c.dns_applied {
                println!("  {:<6}Tablet set the DNS itself", "note");
            }
            if let Some(e) = &c.dns_error {
                println!("  {:<6}setting the DNS failed: {e}", "fail");
            }
        }
        if let Some(sess) = &p.session {
            let quiet = p
                .silent_for
                .map(|q| format!(", nothing from the server for {}", span(q)))
                .unwrap_or_default();
            println!(
                "  {:<6}traffic: {} in, {} out, {} keepalive timeouts, {} reconnects{quiet}",
                if p.silent_for.is_some() { "warn" } else { "ok" },
                bytes(sess.stat("BYTES_IN") as f64),
                bytes(sess.stat("BYTES_OUT") as f64),
                sess.stat("KEEPALIVE_TIMEOUT"),
                sess.stat("N_RECONNECT"),
            );
        }
    }
    Ok(if bad { VPN_DOWN } else { 0 })
}

fn why(args: &[String]) -> Result<i32> {
    let Some(target) = positional(args) else {
        return Err(anyhow!("usage: tablet why <host|url>"));
    };
    let (cfg, _) = Config::load();
    let host = route::host_of(&target);
    match route::why(&cfg, &target) {
        Route::Unknown => {
            println!(
                "{host} is not under any profile's zones, so Tablet has nothing to say about it."
            );
            Ok(0)
        }
        Route::Outside { profile, note } => {
            println!("{host} does not go through {profile}: {note}");
            Ok(0)
        }
        Route::Through { profile, zone } => {
            let _ = ensure_daemon();
            let view = snapshot().and_then(|s| s.profile(&profile).cloned());
            let t = now();
            let Some(p) = view else {
                println!("{host} needs {profile} (zone {zone}); the Tablet daemon did not answer.");
                return Ok(1);
            };
            let state = match p.phase {
                Phase::Held => {
                    let checked = p
                        .checks
                        .as_ref()
                        .map(|c| {
                            format!(
                                ", {}/{} probes answered {}",
                                c.probes.len() - c.failed(),
                                c.probes.len(),
                                ago(t - c.at)
                            )
                        })
                        .unwrap_or_default();
                    format!(
                        "{profile} is connected{checked}. If {host} fails, look at the service."
                    )
                }
                Phase::Off => format!(
                    "{profile} is not connected. This is the VPN, not the service: `tablet need {profile} -- <command>` asks for it."
                ),
                ph => format!(
                    "{profile} is {} since {}{}. This is the VPN, not the service.",
                    ph.label(),
                    when(p.since, t),
                    p.reason
                        .as_deref()
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default()
                ),
            };
            println!("{host} needs {profile} (zone {zone}). {state}");
            Ok(if p.phase == Phase::Held { 0 } else { VPN_DOWN })
        }
    }
}

/// The nearest ancestor that is not a shell or a wrapper: the agent or
/// program that ran `tablet need`.
fn requester() -> String {
    const SKIP: [&str; 10] = [
        "sh", "bash", "zsh", "fish", "dash", "nu", "env", "timeout", "sudo", "tablet",
    ];
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..6 {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        let comm = comm.trim().to_string();
        if comm.is_empty() {
            break;
        }
        if !SKIP.contains(&comm.as_str()) {
            return comm;
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let Some(ppid) = stat
            .rfind(')')
            .and_then(|i| stat[i + 1..].split_whitespace().nth(1))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            break;
        };
        if ppid <= 1 {
            break;
        }
        pid = ppid;
    }
    String::new()
}

#[derive(Debug, PartialEq)]
struct NeedArgs {
    profile: Option<String>,
    reason: String,
    timeout: u64,
    command: Vec<String>,
}

fn parse_need(args: &[String]) -> Result<NeedArgs> {
    let usage = || {
        anyhow!("usage: tablet need [profile] [--reason <text>] [--timeout <secs>] -- <command>...")
    };
    let split = args.iter().position(|a| a == "--").ok_or_else(usage)?;
    let (opts, command) = (&args[..split], &args[split + 1..]);
    if command.is_empty() {
        return Err(usage());
    }
    let mut out = NeedArgs {
        profile: None,
        reason: String::new(),
        timeout: 300,
        command: command.to_vec(),
    };
    let mut it = opts.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--reason" => out.reason = it.next().ok_or_else(usage)?.clone(),
            "--timeout" => {
                out.timeout = it
                    .next()
                    .and_then(|t| t.parse().ok())
                    .ok_or_else(|| anyhow!("--timeout takes a number of seconds"))?
            }
            s if s.starts_with('-') => return Err(anyhow!("unknown option `{s}`")),
            s if out.profile.is_none() => out.profile = Some(s.to_string()),
            _ => return Err(usage()),
        }
    }
    Ok(out)
}

fn need(args: &[String]) -> Result<i32> {
    let n = parse_need(args)?;
    let profile = match n.profile.clone() {
        Some(p) => p,
        None => default_name()?,
    };
    ensure_daemon()?;
    let held = snapshot()
        .and_then(|s| s.profile(&profile).map(|p| p.phase == Phase::Held))
        .unwrap_or(false);
    if !held {
        eprintln!("tablet: {profile} is not connected; asking on the bar (the lamp is amber)");
        let r = tablet_core::protocol::request_patient(&Request::Ask {
            profile: profile.clone(),
            reason: n.reason.clone(),
            command: n.command.join(" "),
            cwd: std::env::current_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            requester: requester(),
            timeout: n.timeout,
        })?;
        if let Some(e) = r.error {
            eprintln!(
                "tablet: {profile} is not connected ({e}). The command needs the VPN, so it did not run."
            );
            return Ok(VPN_DOWN);
        }
        eprintln!(
            "tablet: {profile} is connected; running {}",
            n.command.join(" ")
        );
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&n.command[0])
        .args(&n.command[1..])
        .exec();
    Err(anyhow!("cannot run {}: {err}", n.command[0]))
}

fn history(args: &[String]) -> Result<i32> {
    let count: usize = args
        .iter()
        .position(|a| a == "--count" || a == "-n")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let text = std::fs::read_to_string(paths::history_file()).unwrap_or_default();
    let entries: Vec<tablet_core::model::Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.is_empty() {
        println!("Nothing has happened yet.");
    }
    let t = now();
    for e in entries.iter().rev().take(count) {
        println!(
            "{:<18}{:<10}{:<14}{}",
            when(e.at, t),
            if e.profile.is_empty() {
                "openvpn3"
            } else {
                &e.profile
            },
            e.what,
            e.detail
        );
    }
    Ok(0)
}

fn report(ok: Option<bool>, what: &str, detail: &str) {
    let tag = match ok {
        Some(true) => "ok  ",
        Some(false) => "fail",
        None => "info",
    };
    if detail.is_empty() {
        println!("{tag} {what}");
    } else {
        println!("{tag} {what}: {detail}");
    }
}

fn on_path(program: &str) -> Option<std::path::PathBuf> {
    if program.contains('/') {
        let p = paths::expand(program);
        return p.exists().then_some(p);
    }
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| std::path::Path::new(d).join(program))
        .find(|p| p.is_file())
}

fn doctor(repair: bool) -> Result<i32> {
    if repair {
        return command(Request::Repair);
    }
    let mut failed = false;
    let (cfg, err) = Config::load();
    match err {
        Some(e) => {
            failed = true;
            report(Some(false), "config", &e);
        }
        None if paths::config_file().exists() => report(
            Some(true),
            "config",
            &format!(
                "{}, {} profile{}",
                paths::tilde(&paths::config_file()),
                cfg.profiles.len(),
                if cfg.profiles.len() == 1 { "" } else { "s" }
            ),
        ),
        None => report(
            None,
            "config",
            "no config file: sessions show, but the lamp has nothing to connect",
        ),
    }
    for p in &cfg.profiles {
        let path = p.ovpn_path();
        let ok = path.exists();
        failed |= !ok;
        report(
            Some(ok),
            &format!("profile {}", p.name),
            &if ok {
                paths::tilde(&path)
            } else {
                format!("{} does not exist", paths::tilde(&path))
            },
        );
    }
    let cmd = cfg
        .openvpn3
        .command
        .split_whitespace()
        .next()
        .unwrap_or("openvpn3")
        .to_string();
    match on_path(&cmd) {
        Some(p) => report(Some(true), "openvpn3", &p.display().to_string()),
        None => {
            failed = true;
            report(
                Some(false),
                "openvpn3",
                &format!("`{cmd}` is not installed or not on PATH"),
            );
        }
    }
    let found = tablet_core::doctor::examine(&cfg.doctor, now());
    if found.broken() {
        failed = true;
        report(Some(false), "libraries", &found.summary());
        if cfg.doctor.repair.trim().is_empty() {
            report(
                None,
                "repair",
                "rebuild or reinstall openvpn3 (an AUR build: `paru -S --rebuild openvpn3`), or set doctor.repair",
            );
        } else {
            report(
                None,
                "repair",
                &format!(
                    "`tablet doctor --repair` runs `{}`",
                    cfg.doctor.repair.trim()
                ),
            );
        }
    } else if !found.not_installed {
        report(
            Some(true),
            "libraries",
            &format!(
                "all {} openvpn3 binaries find their libraries",
                found.checked
            ),
        );
    }
    if !found.outdated.is_empty() {
        report(
            None,
            "services",
            &format!(
                "{} run a replaced binary; they restart when idle, or with `sudo pkill -f openvpn3-service-`",
                found.outdated.join(", ")
            ),
        );
    }
    match crate::dbus::connect() {
        Ok(conn) => {
            let mgr = crate::dbus::has_owner(&conn, tablet_core::openvpn::BUS);
            report(
                None,
                "session manager",
                if mgr {
                    "running"
                } else {
                    "not running (openvpn3 starts it with the first session)"
                },
            );
            let resolved = crate::dbus::has_owner(&conn, crate::dbus::RESOLVED);
            report(
                resolved.then_some(true),
                "systemd-resolved",
                if resolved {
                    "running: the tunnel's DNS can be checked"
                } else {
                    "not running: DNS checks are skipped"
                },
            );
        }
        Err(e) => {
            failed = true;
            report(Some(false), "system bus", &format!("{e:#}"));
        }
    }
    if cfg.profiles.iter().any(|p| p.apply_dns) {
        let rule = [
            "/etc/polkit-1/rules.d/50-tablet.rules",
            "/usr/share/polkit-1/rules.d/50-tablet.rules",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists());
        report(
            Some(rule),
            "polkit rule",
            if rule {
                "installed: Tablet may set the tunnel's DNS"
            } else {
                "apply_dns is on but packaging/polkit/50-tablet.rules is not installed"
            },
        );
    }
    let alive = tablet_core::transport::daemon_alive();
    report(
        alive.then_some(true),
        "daemon",
        if alive {
            "running"
        } else {
            "not running (`tablet bar` or a command starts it)"
        },
    );
    match bar_anchor::compositor() {
        Some("Hyprland") => report(
            Some(true),
            "compositor",
            "Hyprland: the drawer opens from the clicked icon",
        ),
        Some(wm) => report(
            Some(true),
            "compositor",
            &format!("{wm}: the drawer opens on the clicked bar"),
        ),
        None => report(
            None,
            "compositor",
            "not Hyprland, sway or niri; the drawer opens centered",
        ),
    }
    match tablet_core::waybar::config_path() {
        Some(p) => {
            let has = std::fs::read_to_string(&p)
                .unwrap_or_default()
                .contains("tablet bar");
            report(
                Some(has),
                "waybar config",
                &format!(
                    "{}: {}",
                    paths::tilde(&p),
                    if has {
                        "has `tablet bar`"
                    } else {
                        "no module runs `tablet bar`"
                    }
                ),
            );
        }
        None => report(None, "waybar config", "not found"),
    }
    let board = paths::exe()
        .and_then(|e| e.parent().map(|d| d.join("tablet-board")))
        .filter(|p| p.exists());
    match board {
        Some(p) => report(Some(true), "drawer", &p.display().to_string()),
        None => report(
            None,
            "drawer",
            "tablet-board not next to tablet; it must be on PATH",
        ),
    }
    Ok(if failed { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// Every command and flag in the usage, from its `  tablet ...` lines.
    fn usage_words() -> Vec<String> {
        let mut words = vec![];
        for line in HELP.lines() {
            let Some(rest) = line.strip_prefix("  tablet ") else {
                continue;
            };
            let mut synopsis = rest.split("  ").next().unwrap_or_default().to_string();
            while let (Some(a), Some(b)) = (synopsis.find('<'), synopsis.find('>')) {
                if b < a {
                    break;
                }
                synopsis.replace_range(a..=b, "");
            }
            for w in synopsis.split([' ', '[', ']', '|']) {
                let w = w.trim().trim_end_matches("...");
                if !w.is_empty()
                    && w != "--"
                    && w.trim_start_matches('-')
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '-')
                {
                    words.push(w.to_string());
                }
            }
        }
        words
    }

    #[test]
    fn completions_and_man_page_know_every_command() {
        let words = usage_words();
        assert!(words.iter().any(|w| w == "need"));
        assert!(words.iter().any(|w| w == "--reason"));
        for (file, text) in [
            (
                "tablet.bash",
                include_str!("../../../packaging/completions/tablet.bash"),
            ),
            (
                "_tablet",
                include_str!("../../../packaging/completions/_tablet"),
            ),
            (
                "tablet.fish",
                include_str!("../../../packaging/completions/tablet.fish"),
            ),
            ("tablet.1", include_str!("../../../packaging/man/tablet.1")),
        ] {
            let text = text.replace("\\-", "-");
            for w in &words {
                let bare = w.trim_start_matches('-');
                assert!(text.contains(bare), "{file} does not mention `{w}`");
            }
        }
    }

    #[test]
    fn need_arguments() {
        assert_eq!(
            parse_need(&s(&["work", "--reason", "nexus", "--", "mvn", "verify"])).unwrap(),
            NeedArgs {
                profile: Some("work".into()),
                reason: "nexus".into(),
                timeout: 300,
                command: s(&["mvn", "verify"])
            }
        );
        assert_eq!(
            parse_need(&s(&["--timeout", "30", "--", "curl", "-s", "--", "x"])).unwrap(),
            NeedArgs {
                profile: None,
                reason: String::new(),
                timeout: 30,
                command: s(&["curl", "-s", "--", "x"])
            }
        );
        assert!(parse_need(&s(&["work"])).is_err());
        assert!(parse_need(&s(&["work", "--"])).is_err());
        assert!(parse_need(&s(&["--bogus", "--", "x"])).is_err());
        assert!(parse_need(&s(&["a", "b", "--", "x"])).is_err());
    }
}
