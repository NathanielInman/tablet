//! `tablet why <host>`: which profile a name depends on, so an agent whose
//! request just failed can tell a dropped tunnel from a broken service.

use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// The name is under one of the profile's zones.
    Through { profile: String, zone: String },
    /// The profile says this name never goes through it.
    Outside { profile: String, note: String },
    /// No profile claims it: not a VPN name, as far as Tablet knows.
    Unknown,
}

/// The host part of what an agent may paste: a URL, `host:port` or a bare
/// name.
pub fn host_of(s: &str) -> String {
    let s = s.trim();
    let s = s.split("://").nth(1).unwrap_or(s);
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s);
    let s = if let Some(rest) = s.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        // `host:port`, or scp's `host:path`; a bare IPv6 address has more
        // than one colon and stays whole.
        match s.split_once(':') {
            Some((h, rest)) if !rest.contains(':') => h,
            _ => s,
        }
    };
    s.trim_end_matches('.').to_ascii_lowercase()
}

/// `host` is `zone` or under it.
pub fn under(host: &str, zone: &str) -> bool {
    let zone = zone
        .trim()
        .trim_start_matches('~')
        .trim_start_matches("*.")
        .trim_matches('.')
        .to_ascii_lowercase();
    if zone.is_empty() {
        return false;
    }
    host == zone || host.ends_with(&format!(".{zone}"))
}

fn matches_outside(host: &str, pattern: &str) -> bool {
    let p = pattern.trim().to_ascii_lowercase();
    match p.strip_prefix("*.") {
        Some(suffix) => host.ends_with(&format!(".{suffix}")),
        None => host == p.trim_end_matches('.'),
    }
}

/// Which profile `host` depends on. `outside` entries win over zones, and
/// the longest zone wins between profiles.
pub fn why(cfg: &Config, target: &str) -> Route {
    let host = host_of(target);
    for p in &cfg.profiles {
        if let Some(o) = p.outside.iter().find(|o| matches_outside(&host, &o.host)) {
            return Route::Outside {
                profile: p.name.clone(),
                note: o.note.clone(),
            };
        }
    }
    let mut best: Option<(&str, &str)> = None;
    for p in &cfg.profiles {
        for z in &p.zones {
            if under(&host, z) && best.is_none_or(|(_, b)| z.len() > b.len()) {
                best = Some((&p.name, z));
            }
        }
    }
    match best {
        Some((profile, zone)) => Route::Through {
            profile: profile.to_string(),
            zone: zone.trim().trim_start_matches('~').to_string(),
        },
        None => Route::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::parse(
            r#"
[[profile]]
name = "work"
ovpn = "~/work.ovpn"
zones = ["corp.example.com", "~example.io"]

[[profile.outside]]
host = "*.gke.example.io"
note = "control planes are public; use the gateway"

[[profile]]
name = "lab"
ovpn = "~/lab.ovpn"
zones = ["lab.corp.example.com"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn hosts_from_what_agents_paste() {
        assert_eq!(
            host_of("https://Grafana.corp.example.com/api/health?x=1"),
            "grafana.corp.example.com"
        );
        assert_eq!(
            host_of("git@git.corp.example.com:group/repo.git"),
            "git.corp.example.com"
        );
        assert_eq!(
            host_of("ssh://git@git.corp.example.com:22/x"),
            "git.corp.example.com"
        );
        assert_eq!(host_of("nexus.example.io:8443"), "nexus.example.io");
        assert_eq!(host_of("[fd00::1]:22"), "fd00::1");
        assert_eq!(host_of("wiki.corp.example.com."), "wiki.corp.example.com");
    }

    #[test]
    fn routes() {
        let c = cfg();
        assert_eq!(
            why(&c, "https://grafana.corp.example.com/d/x"),
            Route::Through {
                profile: "work".into(),
                zone: "corp.example.com".into()
            }
        );
        assert_eq!(
            why(&c, "nexus.example.io"),
            Route::Through {
                profile: "work".into(),
                zone: "example.io".into()
            }
        );
        // The longer zone wins.
        assert_eq!(
            why(&c, "box.lab.corp.example.com"),
            Route::Through {
                profile: "lab".into(),
                zone: "lab.corp.example.com".into()
            }
        );
        assert!(matches!(
            why(&c, "abc.gke.example.io"),
            Route::Outside { .. }
        ));
        assert_eq!(why(&c, "github.com"), Route::Unknown);
        assert_eq!(why(&c, "notcorp.example.com"), Route::Unknown);
        assert!(!under("x.com", ""));
    }
}
