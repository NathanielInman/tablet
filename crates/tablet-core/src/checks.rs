//! Whether a connected tunnel actually works. openvpn3 can report
//! `CONN_CONNECTED` for a session the server stopped answering long ago, so
//! Tablet looks for itself:
//!
//! - **Probes**: a TCP connect to each host a profile names, which also
//!   proves its name resolved through the tunnel's DNS.
//! - **DNS**: the tunnel's link in systemd-resolved has the resolvers and
//!   routing-only domains the profile expects.
//! - **Silence**: openvpn3's `BYTES_IN` stops growing while `BYTES_OUT`
//!   keeps growing. Servers send keepalives every few seconds, so a minute
//!   and a half of nothing back means the other end is gone.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// `host:port` from a probe; port 443 when left out. IPv6 literals go in
/// brackets: `[fd00::1]:22`.
pub fn parse_target(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty".into());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("no closing ]")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| format!("`{p}` is not a port"))?,
            None if after.is_empty() => 443,
            None => return Err("text after ]".into()),
        };
        return Ok((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port: u16 = port
                .parse()
                .map_err(|_| format!("`{port}` is not a port"))?;
            if host.is_empty() {
                return Err("no host".into());
            }
            Ok((host.to_string(), port))
        }
        // A bare IPv6 address.
        Some(_) => Ok((s.to_string(), 443)),
        None => Ok((s.to_string(), 443)),
    }
}

/// One probe's result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Probe {
    /// As the profile wrote it.
    pub target: String,
    pub ok: bool,
    /// Milliseconds to connect (or be refused).
    pub ms: u64,
    /// Why it failed: `no address`, `timed out`, `unreachable`, ...
    pub error: Option<String>,
}

/// Connects to `target` within `timeout`. A refusal still proves the path
/// works (a host answered), so it counts as reachable.
pub fn probe(target: &str, timeout: Duration) -> Probe {
    let start = Instant::now();
    let mut out = Probe {
        target: target.to_string(),
        ..Default::default()
    };
    let (host, port) = match parse_target(target) {
        Ok(t) => t,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    // Name lookup has no timeout of its own, so it runs on a thread that is
    // abandoned if it takes too long.
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let name = format!("{host}:{port}");
            std::thread::spawn(move || {
                let _ = tx.send(name.to_socket_addrs().map(|a| a.collect::<Vec<_>>()));
            });
            match rx.recv_timeout(timeout) {
                Ok(Ok(a)) if !a.is_empty() => a,
                Ok(_) => {
                    out.error = Some("no address (the name did not resolve)".into());
                    out.ms = start.elapsed().as_millis() as u64;
                    return out;
                }
                Err(_) => {
                    out.error = Some("timed out looking up the name".into());
                    out.ms = start.elapsed().as_millis() as u64;
                    return out;
                }
            }
        }
    };
    let mut last = None;
    for a in addrs.iter().take(3) {
        let left = timeout.saturating_sub(start.elapsed());
        if left.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(a, left) {
            Ok(_) => {
                out.ok = true;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                out.ok = true;
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    out.ms = start.elapsed().as_millis() as u64;
    if !out.ok {
        out.error = Some(match last {
            Some(e) if e.kind() == std::io::ErrorKind::TimedOut => "timed out".into(),
            Some(e) => short_io(&e),
            None => "timed out".into(),
        });
    }
    out
}

fn short_io(e: &std::io::Error) -> String {
    let s = e.to_string();
    // "Network is unreachable (os error 101)" -> "network is unreachable".
    let s = s.split(" (os error").next().unwrap_or(&s);
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => s.to_string(),
    }
}

/// What systemd-resolved has on the tunnel's link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Dns {
    pub servers: Vec<String>,
    /// Domains, routing-only ones with a leading `~` as resolvectl prints
    /// them.
    pub domains: Vec<String>,
    pub default_route: Option<bool>,
}

impl Dns {
    /// From the link's `DNS` (`a(iay)`), `Domains` (`a(sb)`) and
    /// `DefaultRoute` properties.
    pub fn from_props(p: &crate::openvpn::Props) -> Dns {
        use crate::openvpn::Value;
        let servers = p
            .get("DNS")
            .map(Value::as_list)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let l = e.as_list();
                match l.get(1) {
                    Some(Value::Bytes(b)) => ip_from_bytes(b).map(|ip| ip.to_string()),
                    _ => None,
                }
            })
            .collect();
        let domains = p
            .get("Domains")
            .map(Value::as_list)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let l = e.as_list();
                let name = l.first()?.as_str()?;
                let routing = l.get(1).and_then(Value::as_bool).unwrap_or(false);
                Some(if routing {
                    format!("~{name}")
                } else {
                    name.to_string()
                })
            })
            .collect();
        Dns {
            servers,
            domains,
            default_route: p.get("DefaultRoute").and_then(Value::as_bool),
        }
    }

    /// What is missing compared with what the profile expects; empty when
    /// it all matches. A zone may be set as a routing-only or a search
    /// domain; either routes its names to the link.
    pub fn missing(&self, servers: &[String], zones: &[String]) -> Vec<String> {
        let mut out = vec![];
        for s in servers {
            let want: Option<IpAddr> = s.trim().parse().ok();
            if !self
                .servers
                .iter()
                .any(|have| have.parse::<IpAddr>().ok() == want)
            {
                out.push(format!("resolver {}", s.trim()));
            }
        }
        for z in zones {
            let z = z.trim().trim_start_matches('~').trim_matches('.');
            if !self.domains.iter().any(|d| {
                d.trim_start_matches('~')
                    .trim_matches('.')
                    .eq_ignore_ascii_case(z)
            }) {
                out.push(format!("zone {z}"));
            }
        }
        out
    }
}

pub fn ip_from_bytes(b: &[u8]) -> Option<IpAddr> {
    match b.len() {
        4 => Some(IpAddr::from([b[0], b[1], b[2], b[3]])),
        16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            Some(IpAddr::from(a))
        }
        _ => None,
    }
}

/// One check of a profile's tunnel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Report {
    /// Unix seconds.
    pub at: f64,
    pub probes: Vec<Probe>,
    /// The link's DNS, when the tunnel has a device resolved knows.
    pub dns: Option<Dns>,
    /// What the link is missing from the profile's `dns` and `zones`.
    pub dns_missing: Vec<String>,
    /// Tablet set the DNS itself this time (`apply_dns`).
    pub dns_applied: bool,
    /// Why setting the DNS failed.
    pub dns_error: Option<String>,
}

impl Report {
    /// Probes ran and every one failed.
    pub fn all_failed(&self) -> bool {
        !self.probes.is_empty() && self.probes.iter().all(|p| !p.ok)
    }

    pub fn failed(&self) -> usize {
        self.probes.iter().filter(|p| !p.ok).count()
    }
}

/// openvpn3's byte counters over time, to see a server go silent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Silence {
    /// `(time, BYTES_IN, BYTES_OUT)` when `BYTES_IN` last grew.
    heard: Option<(f64, i64, i64)>,
    last_out: i64,
}

impl Silence {
    /// Records a reading. Counters that go backwards (a new session, a
    /// reconnect) start over.
    pub fn sample(&mut self, t: f64, bytes_in: i64, bytes_out: i64) {
        match self.heard {
            Some((_, i, o)) if bytes_in == i && bytes_out >= o => {}
            _ => self.heard = Some((t, bytes_in, bytes_out)),
        }
        self.last_out = bytes_out;
    }

    /// Seconds the server has said nothing while we kept sending, or `None`
    /// while it is talking (or nothing went out either, which is an idle
    /// tunnel on a server that sends no keepalives, not a dead one).
    pub fn silent_for(&self, now: f64) -> Option<f64> {
        let (t, _, out) = self.heard?;
        (self.last_out > out).then_some(now - t)
    }

    pub fn reset(&mut self) {
        *self = Silence::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(
            parse_target("git.corp.example.com:22"),
            Ok(("git.corp.example.com".into(), 22))
        );
        assert_eq!(
            parse_target("wiki.corp.example.com"),
            Ok(("wiki.corp.example.com".into(), 443))
        );
        assert_eq!(parse_target("10.0.0.5:8080"), Ok(("10.0.0.5".into(), 8080)));
        assert_eq!(parse_target("[fd00::1]:22"), Ok(("fd00::1".into(), 22)));
        assert_eq!(parse_target("[fd00::1]"), Ok(("fd00::1".into(), 443)));
        assert_eq!(parse_target("fd00::1"), Ok(("fd00::1".into(), 443)));
        assert!(parse_target("host:http").is_err());
        assert!(parse_target(":22").is_err());
        assert!(parse_target("").is_err());
    }

    #[test]
    fn probes_a_local_listener_and_a_closed_port() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let p = probe(&format!("127.0.0.1:{port}"), Duration::from_secs(2));
        assert!(p.ok, "{p:?}");
        drop(l);
        // Refused: the host answered, so the path works.
        let p = probe(&format!("127.0.0.1:{port}"), Duration::from_secs(2));
        assert!(p.ok, "{p:?}");
        let p = probe("name.invalid:443", Duration::from_secs(3));
        assert!(!p.ok);
        assert!(p.error.is_some());
    }

    #[test]
    fn reads_a_recorded_link() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/resolved-link.json")).unwrap();
        let props = match crate::openvpn::from_busctl(
            &serde_json::json!({"type": v["type"], "data": v["data"][0]}),
        ) {
            crate::openvpn::Value::Dict(d) => d,
            _ => panic!(),
        };
        let d = Dns::from_props(&props);
        assert_eq!(d.servers, vec!["10.8.0.2", "10.8.0.3"]);
        assert_eq!(d.domains, vec!["~corp.example.com", "~corp.example.io"]);
        assert_eq!(d.default_route, Some(false));
        assert!(
            d.missing(&["10.8.0.2".into()], &["corp.example.com".into()])
                .is_empty()
        );
        assert_eq!(
            d.missing(
                &["10.8.0.9".into()],
                &["other.example.com".into(), "~corp.example.io".into()]
            ),
            vec!["resolver 10.8.0.9", "zone other.example.com"]
        );
        assert!(Dns::default().missing(&[], &[]).is_empty());
    }

    #[test]
    fn silence() {
        let mut s = Silence::default();
        s.sample(0.0, 100, 100);
        assert_eq!(s.silent_for(10.0), None);
        // Bytes go out, nothing comes back.
        s.sample(30.0, 100, 160);
        s.sample(90.0, 100, 220);
        assert_eq!(s.silent_for(95.0), Some(95.0));
        // The server answers.
        s.sample(100.0, 140, 230);
        assert_eq!(s.silent_for(105.0), None);
        // An idle tunnel: nothing either way.
        s.sample(400.0, 140, 230);
        assert_eq!(s.silent_for(500.0), None);
        // A reconnect resets the counters.
        s.sample(500.0, 10, 10);
        assert_eq!(s.silent_for(600.0), None);
    }
}
