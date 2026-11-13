//! The waybar line: one lamp for every profile, the most urgent state
//! first.

use crate::config::Config;
use crate::model::{Phase, Snapshot};
use crate::protocol::BarLine;
use crate::text::{escape, span};

/// The lamp's state, most urgent first: a request waiting on you, a
/// sign-in, something red, openvpn3 broken, a stale tunnel, one on its way
/// up, a held one missing its DNS, a held one, nothing.
pub fn lamp(s: &Snapshot) -> &'static str {
    let any = |ph: Phase| s.profiles.iter().any(|p| p.phase == ph);
    let held = any(Phase::Held);
    if !s.asks.is_empty() {
        "asking"
    } else if any(Phase::SignIn) {
        "signin"
    } else if any(Phase::Failed) {
        "failed"
    } else if any(Phase::Dropped) {
        "dropped"
    } else if s.doctor.broken() && !held {
        "broken"
    } else if any(Phase::Stale) {
        "stale"
    } else if any(Phase::Starting) || any(Phase::Reconnecting) {
        "connecting"
    } else if held
        && s.profiles.iter().any(|p| {
            p.phase == Phase::Held && p.checks.as_ref().is_some_and(|c| !c.dns_missing.is_empty())
        })
    {
        "degraded"
    } else if held
        || s.others
            .iter()
            .any(|o| o.status.kind() == crate::openvpn::Kind::Connected)
    {
        "held"
    } else {
        "off"
    }
}

fn word(lamp: &str) -> &'static str {
    match lamp {
        "asking" => "asked",
        "signin" => "sign in",
        "failed" => "failed",
        "dropped" => "dropped",
        "broken" => "rebuild",
        "stale" => "stale",
        "connecting" => "\u{2026}",
        "degraded" => "dns",
        _ => "",
    }
}

/// The waybar line for a snapshot.
pub fn line(s: &Snapshot, cfg: &Config) -> BarLine {
    let lamp = s.lamp.as_str();
    let icon = match lamp {
        "held" | "degraded" => &cfg.bar.icon_on,
        "broken" => &cfg.bar.icon_broken,
        _ => &cfg.bar.icon_off,
    };
    let w = word(lamp);
    let text = if cfg.bar.words && !w.is_empty() {
        if icon.is_empty() {
            w.to_string()
        } else {
            format!("{icon} {w}")
        }
    } else {
        icon.clone()
    };
    let mut class = vec![lamp.to_string()];
    if matches!(lamp, "asking" | "signin" | "connecting") {
        class.push("pulse".into());
    }
    BarLine {
        text: escape(&text),
        class,
        alt: lamp.to_string(),
        tooltip: escape(&tooltip(s)),
    }
}

/// What each profile is doing, one line each, then openvpn3's health and
/// the clicks.
pub fn tooltip(s: &Snapshot) -> String {
    let mut lines = vec![];
    for a in &s.asks {
        lines.push(format!(
            "{} wants {}: {}",
            if a.requester.is_empty() {
                "an agent"
            } else {
                &a.requester
            },
            a.profile,
            if a.reason.is_empty() {
                "no reason given"
            } else {
                &a.reason
            }
        ));
    }
    for p in &s.profiles {
        let mut l = format!("{}: {}", p.name, p.phase.label());
        match p.phase {
            Phase::Held => {
                if let Some(at) = p.connected_at {
                    l.push_str(&format!(" {}", span(s.now - at)));
                }
                if let Some(dev) = p
                    .session
                    .as_ref()
                    .map(|x| x.device.as_str())
                    .filter(|d| !d.is_empty())
                {
                    l.push_str(&format!(" \u{b7} {dev}"));
                }
                if let Some(c) = &p.checks {
                    if !c.probes.is_empty() {
                        l.push_str(&format!(
                            " \u{b7} {}/{} probes ok {}",
                            c.probes.len() - c.failed(),
                            c.probes.len(),
                            crate::text::ago(s.now - c.at)
                        ));
                    }
                    if !c.dns_missing.is_empty() {
                        l.push_str(&format!(" \u{b7} DNS missing {}", c.dns_missing.join(", ")));
                    }
                }
            }
            Phase::Off => {}
            _ => {
                if let Some(r) = &p.reason {
                    l.push_str(&format!(": {r}"));
                }
            }
        }
        lines.push(l);
    }
    for o in &s.others {
        lines.push(format!(
            "{}: {} (no profile)",
            o.config_name.rsplit('/').next().unwrap_or(&o.config_name),
            o.status.describe()
        ));
    }
    if s.doctor.broken() || s.doctor.not_installed {
        lines.push(format!("{} (tablet doctor)", s.doctor.summary()));
    }
    if let Some(e) = &s.config_error {
        lines.push(format!("config: {e}"));
    }
    if s.profiles.is_empty() && s.others.is_empty() {
        lines.push("No profiles yet: add one to ~/.config/tablet/config.toml".into());
    }
    lines.push(
        "Click connects or disconnects, right-click opens the drawer, middle-click checks".into(),
    );
    lines.join("\n")
}

/// What the bar shows while the daemon cannot be reached.
pub fn offline(cfg: &Config) -> BarLine {
    BarLine {
        text: escape(&cfg.bar.icon_off),
        class: vec!["offline".into()],
        alt: "offline".into(),
        tooltip: "The Tablet daemon is not running".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProfileView;

    fn snap(phase: Phase) -> Snapshot {
        Snapshot {
            now: 100.0,
            profiles: vec![ProfileView {
                name: "work".into(),
                phase,
                connected_at: Some(0.0),
                reason: Some("the server refused the sign-in".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn lines() {
        let cfg = Config::default();
        let mut s = snap(Phase::Held);
        s.lamp = lamp(&s).into();
        let l = line(&s, &cfg);
        assert_eq!(l.text, "\u{f0318}");
        assert_eq!(l.class, vec!["held"]);
        assert!(l.tooltip.starts_with("work: held 1m"));

        let mut s = snap(Phase::Failed);
        s.lamp = lamp(&s).into();
        let l = line(&s, &cfg);
        assert_eq!(l.text, "\u{f0319} failed");
        assert!(
            l.tooltip
                .contains("work: failed: the server refused the sign-in")
        );

        let mut s = snap(Phase::Starting);
        s.lamp = lamp(&s).into();
        assert_eq!(line(&s, &cfg).class, vec!["connecting", "pulse"]);

        let mut quiet = Config::default();
        quiet.bar.words = false;
        let mut s = snap(Phase::Stale);
        s.lamp = lamp(&s).into();
        assert_eq!(line(&s, &quiet).text, "\u{f0319}");
        assert_eq!(line(&s, &quiet).alt, "stale");
    }

    #[test]
    fn phases_spell_the_same_everywhere() {
        for ph in [
            Phase::Off,
            Phase::Starting,
            Phase::SignIn,
            Phase::Held,
            Phase::Reconnecting,
            Phase::Stale,
            Phase::Failed,
            Phase::Dropped,
        ] {
            assert_eq!(
                serde_json::to_string(&ph).unwrap(),
                format!("\"{}\"", ph.as_str())
            );
        }
    }

    #[test]
    fn urgency_order() {
        let mut s = snap(Phase::Held);
        s.profiles.push(ProfileView {
            name: "lab".into(),
            phase: Phase::Dropped,
            ..Default::default()
        });
        assert_eq!(lamp(&s), "dropped");
        s.asks.push(Default::default());
        assert_eq!(lamp(&s), "asking");
        let mut s = snap(Phase::Off);
        s.doctor.missing.push(("a".into(), "b".into()));
        assert_eq!(lamp(&s), "broken");
        // A held tunnel keeps working on the old binaries.
        let mut s = snap(Phase::Held);
        s.doctor.missing.push(("a".into(), "b".into()));
        assert_eq!(lamp(&s), "held");
        assert_eq!(lamp(&snap(Phase::Off)), "off");
    }
}
