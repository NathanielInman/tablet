//! `~/.config/tablet/config.toml`. Every key is optional; with no file
//! Tablet still shows every openvpn3 session, it just has no profile to
//! connect from the lamp.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bar: BarConfig,
    pub checks: ChecksConfig,
    pub openvpn3: Openvpn3Config,
    pub doctor: DoctorConfig,
    pub drawer: DrawerConfig,
    pub theme: ThemeConfig,
    #[serde(rename = "profile")]
    pub profiles: Vec<Profile>,
}

/// One VPN you connect to: an `.ovpn` file and what should work once it is
/// up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// A short name for the bar, the drawer and `tablet need <name>`.
    pub name: String,
    /// The `.ovpn` file `openvpn3 session-start --config` is given. A
    /// session started from it by hand is recognized too.
    pub ovpn: String,
    /// What a click on the lamp connects. Without one, the first profile.
    pub default: bool,
    /// Resolvers the tunnel's link should use in systemd-resolved. Checked
    /// while connected; applied too with `apply_dns`.
    pub dns: Vec<String>,
    /// Domains whose names resolve through the tunnel (`corp.example.com`
    /// covers every name under it). Checked as routing-only domains, and
    /// `tablet why` answers with this profile for names under them.
    pub zones: Vec<String>,
    /// Set `dns` and `zones` on the tunnel's link when openvpn3 did not.
    /// Needs the polkit rule in `packaging/polkit`.
    pub apply_dns: bool,
    /// Hosts that must answer through the tunnel, `host:port` (port 443
    /// when left out). A TCP connect is tried; a refusal still counts as
    /// reachable.
    pub probes: Vec<String>,
    /// Hosts that never go through this tunnel, with what to do instead.
    /// `tablet why` says so rather than blaming the VPN.
    pub outside: Vec<Outside>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Outside {
    /// A host name, or `*.suffix` for every name under a domain.
    pub host: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    /// Glyph while a tunnel is held.
    pub icon_on: String,
    /// Glyph while nothing is connected, and beside the words for trouble.
    pub icon_off: String,
    /// Glyph while openvpn3 itself is broken.
    pub icon_broken: String,
    /// A word beside the glyph when the tunnel is not simply on or off
    /// (`sign in`, `stale`, `dropped`, ...).
    pub words: bool,
    /// The layer-shell namespace of the bar surface, used to find its edge.
    pub namespace: String,
}

impl Default for BarConfig {
    fn default() -> Self {
        BarConfig {
            icon_on: "\u{f0318}".into(),
            icon_off: "\u{f0319}".into(),
            icon_broken: "\u{f0ad}".into(),
            words: true,
            namespace: "waybar".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ChecksConfig {
    /// Seconds between checks (probes and DNS) while a tunnel is held.
    /// 0 checks only on connect, after a resume, and when asked.
    pub every: u64,
    /// Seconds one probe may take.
    pub timeout: u64,
    /// Seconds without a byte from the server, while bytes keep going out,
    /// before a held tunnel counts as stale. Servers send keepalives every
    /// few seconds, so silence this long means the other end is gone.
    pub silence: u64,
    /// Seconds a tunnel may spend connecting or reconnecting before it
    /// counts as failed.
    pub grace: u64,
}

impl Default for ChecksConfig {
    fn default() -> Self {
        ChecksConfig {
            every: 120,
            timeout: 4,
            silence: 90,
            grace: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Openvpn3Config {
    /// The openvpn3 CLI, used to start sessions (it handles the sign-in).
    pub command: String,
    /// Seconds `session-start` may run before it is stopped.
    pub start_timeout: u64,
    /// Opens a sign-in page again from the drawer; the URL is its last
    /// argument. openvpn3 opens the first one itself.
    pub opener: String,
}

impl Default for Openvpn3Config {
    fn default() -> Self {
        Openvpn3Config {
            command: "openvpn3".into(),
            start_timeout: 300,
            opener: "xdg-open".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DoctorConfig {
    /// Files, or directories of executables, checked for libraries the
    /// dynamic linker can no longer find.
    pub binaries: Vec<String>,
    /// Repairs openvpn3 after `tablet doctor` finds it broken (for an AUR
    /// build: `paru -S --rebuild openvpn3`). Empty: the drawer only says
    /// what is wrong.
    pub repair: String,
    /// Runs `repair` so you can watch it and answer prompts, with the
    /// command as its last argument (`kitty --hold sh -c`). Empty runs it
    /// in the background with its output in the daemon log.
    pub terminal: String,
}

impl Default for DoctorConfig {
    fn default() -> Self {
        DoctorConfig {
            binaries: vec!["/usr/bin/openvpn3".into(), "/usr/lib/openvpn3-linux".into()],
            repair: String::new(),
            terminal: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DrawerConfig {
    /// Panel width in logical pixels.
    pub width: f64,
    /// Largest height as a fraction of the monitor.
    pub max_height: f64,
    /// Gap between the bar and the panel.
    pub gap: f64,
    /// Show the server's address and port. Off keeps them off screen
    /// shares.
    pub addresses: bool,
}

impl Default for DrawerConfig {
    fn default() -> Self {
        DrawerConfig {
            width: 400.0,
            max_height: 0.85,
            gap: 8.0,
            addresses: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// `waybar` reads the bar's style.css for font and colors; `none` uses
    /// the built-in theme.
    pub from: String,
    /// Overrides the accent color.
    pub accent: Option<String>,
    /// Overrides the font family.
    pub font: Option<String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            from: "waybar".into(),
            accent: None,
            font: None,
        }
    }
}

/// Expands a leading `~/` in a command line.
pub fn expand_command(c: &str) -> String {
    let c = c.trim();
    match c.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", crate::paths::home().display()),
        None => c.to_string(),
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let c: Config =
            toml::from_str(text).map_err(|e| e.to_string().trim().replace('\n', " "))?;
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for p in &self.profiles {
            let name = p.name.trim();
            if name.is_empty() {
                return Err("a [[profile]] has no name".into());
            }
            if name.contains(char::is_whitespace) {
                return Err(format!("profile name `{name}` has a space in it"));
            }
            if !seen.insert(name.to_string()) {
                return Err(format!("two profiles are called `{name}`"));
            }
            if p.ovpn.trim().is_empty() {
                return Err(format!("profile `{name}` has no ovpn file"));
            }
            for probe in &p.probes {
                crate::checks::parse_target(probe)
                    .map_err(|e| format!("profile `{name}`: probe `{probe}`: {e}"))?;
            }
            for s in &p.dns {
                s.trim()
                    .parse::<std::net::IpAddr>()
                    .map_err(|_| format!("profile `{name}`: `{s}` is not an IP address"))?;
            }
        }
        if self.profiles.iter().filter(|p| p.default).count() > 1 {
            return Err("more than one profile has default = true".into());
        }
        Ok(())
    }

    /// The config and, when the file is broken, why (the defaults are used).
    pub fn load() -> (Config, Option<String>) {
        let path = crate::paths::config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => match Config::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// What a click on the lamp connects.
    pub fn default_profile(&self) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.default)
            .or_else(|| self.profiles.first())
    }
}

impl Profile {
    /// The `.ovpn` path with `~` expanded.
    pub fn ovpn_path(&self) -> std::path::PathBuf {
        crate::paths::expand(&self.ovpn)
    }

    /// An openvpn3 session belongs to this profile: it was started from the
    /// same file (openvpn3 names a session's config by the path it was
    /// given) or from a config imported under the profile's name.
    pub fn owns(&self, config_name: &str) -> bool {
        let c = config_name.trim();
        if c.is_empty() {
            return false;
        }
        if c == self.name || c == self.ovpn.trim() {
            return true;
        }
        let path = self.ovpn_path();
        if std::path::Path::new(c) == path {
            return true;
        }
        // A session started from a relative path or another link to the
        // same file.
        match (std::fs::canonicalize(c), std::fs::canonicalize(&path)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn the_sample_config_parses() {
        let sample = include_str!("../../../packaging/examples/config.toml");
        let c = Config::parse(sample).unwrap();
        assert_eq!(c.profiles.len(), 1);
        assert_eq!(c.default_profile().unwrap().name, "work");
    }

    #[test]
    fn the_readme_config_parses() {
        let readme = include_str!("../../../README.md");
        let config = &readme[readme.find("## Config").unwrap()..];
        let start = config.find("```toml\n").unwrap() + "```toml\n".len();
        let block = &config[start..start + config[start..].find("```").unwrap()];
        let c = Config::parse(block).unwrap();
        assert!(!c.profiles.is_empty());
    }

    #[test]
    fn profiles() {
        let c = Config::parse(
            r#"
[[profile]]
name = "work"
ovpn = "~/work.ovpn"
zones = ["corp.example.com"]
probes = ["git.corp.example.com:22", "wiki.corp.example.com"]

[[profile.outside]]
host = "*.cloud.example.com"
note = "public, use the gateway"

[[profile]]
name = "lab"
ovpn = "/etc/openvpn/lab.ovpn"
default = true
"#,
        )
        .unwrap();
        assert_eq!(c.profiles.len(), 2);
        assert_eq!(c.profiles[0].outside[0].host, "*.cloud.example.com");
        assert_eq!(c.default_profile().unwrap().name, "lab");
        let work = c.profile("work").unwrap();
        assert!(work.owns("work"));
        assert!(work.owns("~/work.ovpn"));
        assert!(work.owns(&crate::paths::home().join("work.ovpn").display().to_string()));
        assert!(!work.owns("/home/someone/other.ovpn"));
        assert!(!work.owns(""));
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(Config::parse("[bar]\nicon = \"x\"\n").is_err());
        assert!(Config::parse("[[profile]]\novpn = \"a\"\n").is_err());
        assert!(Config::parse("[[profile]]\nname = \"a b\"\novpn = \"a\"\n").is_err());
        assert!(
            Config::parse(
                "[[profile]]\nname = \"a\"\novpn = \"x\"\n[[profile]]\nname = \"a\"\novpn = \"y\"\n"
            )
            .is_err()
        );
        assert!(Config::parse("[[profile]]\nname = \"a\"\n").is_err());
        assert!(
            Config::parse("[[profile]]\nname = \"a\"\novpn = \"x\"\ndns = [\"resolver\"]\n")
                .is_err()
        );
        assert!(
            Config::parse("[[profile]]\nname = \"a\"\novpn = \"x\"\nprobes = [\"host:http\"]\n")
                .is_err()
        );
    }
}
