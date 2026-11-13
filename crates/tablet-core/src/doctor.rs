//! Is openvpn3 itself fit to run? Two things break it on a rolling
//! distribution, and both look like "the VPN button does nothing":
//!
//! - **A library it was built against is gone.** An update replaces
//!   `libprotobuf.so.36.0.0` with `.36.1.0`, and an openvpn3 built from
//!   source (the AUR package) still asks for the old name. Its backend then
//!   fails to start: "New tunnel did not respond". Tablet reads each
//!   binary's `DT_NEEDED` entries and looks for them where the dynamic
//!   linker would.
//! - **Its services run code that was replaced.** After a rebuild the
//!   long-running D-Bus services (`openvpn3-service-sessionmgr` and the
//!   rest) keep running the old binaries until they restart. A process that
//!   started before its binary's file was written is one of those.
//!
//! Everything here reads files; nothing runs a program.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The doctor's findings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Doctor {
    /// Unix seconds.
    pub at: f64,
    /// How many binaries were checked.
    pub checked: usize,
    /// `(binary, library)` for each library a binary needs and cannot find.
    pub missing: Vec<(String, String)>,
    /// openvpn3 services running a binary that has since been replaced.
    pub outdated: Vec<String>,
    /// No openvpn3 binary was found where the config says to look.
    pub not_installed: bool,
}

impl Doctor {
    /// openvpn3 cannot start a tunnel until it is rebuilt or reinstalled.
    pub fn broken(&self) -> bool {
        !self.missing.is_empty()
    }

    /// The libraries that are missing, each once.
    pub fn missing_libs(&self) -> Vec<String> {
        let mut v: Vec<String> = self.missing.iter().map(|(_, l)| l.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// One sentence for the bar's tooltip and the drawer.
    pub fn summary(&self) -> String {
        if self.not_installed {
            return "openvpn3 is not installed".into();
        }
        if self.broken() {
            let libs = self.missing_libs();
            let bins: std::collections::BTreeSet<&str> = self
                .missing
                .iter()
                .map(|(b, _)| b.rsplit('/').next().unwrap_or(b))
                .collect();
            return format!(
                "openvpn3 cannot start: {} {} missing for {}",
                libs.join(", "),
                if libs.len() == 1 { "is" } else { "are" },
                bins.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        if !self.outdated.is_empty() {
            return format!(
                "{} still {} the old openvpn3; it restarts with the next session",
                self.outdated.join(", "),
                if self.outdated.len() == 1 {
                    "runs"
                } else {
                    "run"
                }
            );
        }
        format!("openvpn3 is fine ({} binaries link)", self.checked)
    }
}

/// The `DT_NEEDED` sonames and the `DT_RUNPATH`/`DT_RPATH` directories of a
/// 64- or 32-bit little-endian ELF file. `None` when it is not one (a
/// script, a static binary has no needs and returns empty lists).
pub fn elf_needs(data: &[u8]) -> Option<(Vec<String>, Vec<String>)> {
    if data.len() < 64 || &data[..4] != b"\x7fELF" || data[5] != 1 {
        return None;
    }
    let is64 = match data[4] {
        2 => true,
        1 => false,
        _ => return None,
    };
    let u16_at = |o: usize| {
        data.get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as u64)
    };
    let u32_at = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64)
    };
    let u64_at = |o: usize| {
        data.get(o..o + 8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()))
    };
    let word = |o: usize| if is64 { u64_at(o) } else { u32_at(o) };
    let (phoff, phentsize, phnum) = if is64 {
        (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?)
    } else {
        (u32_at(0x1c)?, u16_at(0x2a)?, u16_at(0x2c)?)
    };
    // Program headers: PT_LOAD maps addresses to file offsets, PT_DYNAMIC
    // is the dynamic section.
    let mut loads = vec![];
    let mut dynamic = None;
    for i in 0..phnum {
        let h = (phoff + i * phentsize) as usize;
        let ty = u32_at(h)?;
        let (offset, vaddr, filesz) = if is64 {
            (u64_at(h + 8)?, u64_at(h + 16)?, u64_at(h + 32)?)
        } else {
            (u32_at(h + 4)?, u32_at(h + 8)?, u32_at(h + 16)?)
        };
        match ty {
            1 => loads.push((vaddr, offset, filesz)),
            2 => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let Some((dyn_off, dyn_size)) = dynamic else {
        return Some((vec![], vec![]));
    };
    let entry = if is64 { 16 } else { 8 };
    let mut needed = vec![];
    let mut paths = vec![];
    let mut strtab = None;
    let mut i = 0;
    while i + entry <= dyn_size {
        let o = (dyn_off + i) as usize;
        let tag = word(o)?;
        let val = word(o + entry as usize / 2)?;
        match tag {
            0 => break,
            1 => needed.push(val),
            5 => strtab = Some(val),
            15 | 29 => paths.push(val),
            _ => {}
        }
        i += entry;
    }
    let strtab = strtab?;
    let to_file = |addr: u64| {
        loads
            .iter()
            .find(|(v, _, sz)| addr >= *v && addr < v + sz)
            .map(|(v, off, _)| addr - v + off)
    };
    let base = to_file(strtab)? as usize;
    let string = |off: u64| -> Option<String> {
        let start = base + off as usize;
        let rest = data.get(start..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    };
    let needed = needed.into_iter().filter_map(string).collect();
    let paths = paths
        .into_iter()
        .filter_map(string)
        .flat_map(|p| p.split(':').map(String::from).collect::<Vec<_>>())
        .filter(|p| !p.is_empty())
        .collect();
    Some((needed, paths))
}

/// Directories the dynamic linker searches: those listed in
/// `/etc/ld.so.conf` (following its `include` lines) and the defaults.
pub fn lib_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![];
    read_ld_conf(root, &root.join("etc/ld.so.conf"), &mut dirs, 0);
    for d in [
        "lib",
        "usr/lib",
        "lib64",
        "usr/lib64",
        "usr/local/lib",
        "lib/x86_64-linux-gnu",
        "usr/lib/x86_64-linux-gnu",
        "lib/aarch64-linux-gnu",
        "usr/lib/aarch64-linux-gnu",
    ] {
        dirs.push(root.join(d));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

fn read_ld_conf(root: &Path, file: &Path, dirs: &mut Vec<PathBuf>, depth: u8) {
    if depth > 4 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line.strip_prefix("include") {
            for f in glob_conf(root, pattern.trim()) {
                read_ld_conf(root, &f, dirs, depth + 1);
            }
        } else {
            dirs.push(root.join(line.trim_start_matches('/')));
        }
    }
}

/// `include /etc/ld.so.conf.d/*.conf`: the only glob ld.so.conf uses is a
/// `*` in the file name.
fn glob_conf(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let p = root.join(pattern.trim_start_matches('/'));
    let (Some(dir), Some(name)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) else {
        return vec![];
    };
    let (pre, post) = name.split_once('*').unwrap_or((name, ""));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            f.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with(pre) && n.ends_with(post) && n.len() >= pre.len() + post.len()
            })
        })
        .collect();
    out.sort();
    out
}

/// The executables the config names: files as they are, directories by
/// their regular files.
pub fn binaries(paths: &[String]) -> Vec<PathBuf> {
    let mut out = vec![];
    for p in paths {
        let p = crate::paths::expand(p);
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                let mut files: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|f| f.is_file())
                    .collect();
                files.sort();
                out.extend(files);
            }
        } else if p.is_file() {
            out.push(p);
        }
    }
    out
}

/// Libraries `binary` needs that are nowhere the linker would look.
pub fn missing_for(binary: &Path, dirs: &[PathBuf]) -> Vec<String> {
    let Ok(data) = std::fs::read(binary) else {
        return vec![];
    };
    let Some((needed, runpath)) = elf_needs(&data) else {
        return vec![];
    };
    let origin = binary.parent().unwrap_or(Path::new("/"));
    let mut search: Vec<PathBuf> = runpath
        .iter()
        .map(|p| PathBuf::from(p.replace("$ORIGIN", &origin.display().to_string())))
        .collect();
    search.extend(dirs.iter().cloned());
    needed
        .into_iter()
        .filter(|lib| {
            // A needed entry with a slash is a path of its own.
            if lib.contains('/') {
                return !Path::new(lib).exists();
            }
            !search.iter().any(|d| d.join(lib).exists())
        })
        .collect()
}

/// The start time of a process in Unix seconds, from `/proc/<pid>/stat`
/// (field 22, clock ticks after boot) and the boot time (`btime` in
/// `/proc/stat`).
pub fn started(stat: &str, btime: f64, ticks: f64) -> Option<f64> {
    // The command name is in parentheses and may hold spaces.
    let after = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // After the name, field 3 (state) is index 0, so field 22 is index 19.
    let t: f64 = fields.get(19)?.parse().ok()?;
    Some(btime + t / ticks)
}

pub fn boot_time(proc_stat: &str) -> Option<f64> {
    proc_stat
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

/// openvpn3 services (any `openvpn3-service-*` process) whose binary was
/// written after they started, by name.
pub fn outdated_services(proc_root: &Path) -> Vec<String> {
    let Some(btime) = std::fs::read_to_string(proc_root.join("stat"))
        .ok()
        .as_deref()
        .and_then(boot_time)
    else {
        return vec![];
    };
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let ticks = if ticks > 0.0 { ticks } else { 100.0 };
    let Ok(rd) = std::fs::read_dir(proc_root) else {
        return vec![];
    };
    let mut out = vec![];
    for e in rd.flatten() {
        if e.file_name()
            .to_str()
            .is_none_or(|n| !n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let Ok(cmdline) = std::fs::read(e.path().join("cmdline")) else {
            continue;
        };
        let argv0 = String::from_utf8_lossy(cmdline.split(|b| *b == 0).next().unwrap_or_default())
            .into_owned();
        let name = argv0.rsplit('/').next().unwrap_or_default().to_string();
        if !name.starts_with("openvpn3-service-") || !argv0.starts_with('/') {
            continue;
        }
        let Some(start) = std::fs::read_to_string(e.path().join("stat"))
            .ok()
            .and_then(|s| started(&s, btime, ticks))
        else {
            continue;
        };
        let written = std::fs::metadata(&argv0)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        // A second of slack for clocks and file systems that round.
        if written.is_some_and(|w| w > start + 1.0) && !out.contains(&name) {
            out.push(name);
        }
    }
    out.sort();
    out
}

/// Runs every check.
pub fn examine(cfg: &crate::config::DoctorConfig, now: f64) -> Doctor {
    let bins = binaries(&cfg.binaries);
    let dirs = lib_dirs(Path::new("/"));
    let mut missing = vec![];
    for b in &bins {
        for lib in missing_for(b, &dirs) {
            missing.push((b.display().to_string(), lib));
        }
    }
    Doctor {
        at: now,
        checked: bins.len(),
        missing,
        outdated: outdated_services(Path::new("/proc")),
        // An empty list turns the check off; a list that finds nothing
        // means openvpn3 is not there.
        not_installed: !cfg.binaries.is_empty() && bins.is_empty(),
    }
}

/// When `/etc/ld.so.cache` last changed: package managers run `ldconfig`
/// after installing libraries, so a change here is the moment to look
/// again.
pub fn ld_cache_stamp() -> Option<std::time::SystemTime> {
    std::fs::metadata("/etc/ld.so.cache")
        .and_then(|m| m.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_test_binary() {
        let exe = std::env::current_exe().unwrap();
        let data = std::fs::read(&exe).unwrap();
        let (needed, _) = elf_needs(&data).expect("an ELF file");
        assert!(
            needed.iter().any(|n| n.starts_with("libc.so")),
            "{needed:?}"
        );
        // Everything it needs is found, or it would not be running.
        let dirs = lib_dirs(Path::new("/"));
        assert!(missing_for(&exe, &dirs).is_empty());
        assert!(elf_needs(b"#!/bin/sh\necho hi\n").is_none());
    }

    #[test]
    fn a_missing_library_is_found_missing() {
        let exe = std::env::current_exe().unwrap();
        // No directory to look in: every library is "missing".
        let none = missing_for(&exe, &[]);
        assert!(none.iter().any(|n| n.starts_with("libc.so")));
    }

    #[test]
    fn ld_conf_includes() {
        let root = std::env::temp_dir().join(format!("tablet-ld-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("etc/ld.so.conf.d")).unwrap();
        std::fs::write(
            root.join("etc/ld.so.conf"),
            "# comment\ninclude /etc/ld.so.conf.d/*.conf\n/opt/one\n",
        )
        .unwrap();
        std::fs::write(
            root.join("etc/ld.so.conf.d/a.conf"),
            "/opt/two # trailing\n",
        )
        .unwrap();
        std::fs::write(root.join("etc/ld.so.conf.d/skip.txt"), "/opt/three\n").unwrap();
        let dirs = lib_dirs(&root);
        assert!(dirs.contains(&root.join("opt/one")));
        assert!(dirs.contains(&root.join("opt/two")));
        assert!(!dirs.contains(&root.join("opt/three")));
        assert!(dirs.contains(&root.join("usr/lib")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn process_start_times() {
        let stat = "1480 (openvpn3-servic) S 1 1480 1480 0 -1 4194560 3020 0 0 0 108 51 0 0 20 0 3 0 1234 233472000 3075 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0";
        assert_eq!(started(stat, 1_000_000.0, 100.0), Some(1_000_012.34));
        let odd = "7 (a (b) c) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 500 0 0";
        assert_eq!(started(odd, 0.0, 100.0), Some(5.0));
        assert_eq!(
            boot_time("cpu 1 2 3\nbtime 1794000000\nprocesses 9\n"),
            Some(1_794_000_000.0)
        );
        // This process started after boot and before now.
        let me = std::fs::read_to_string("/proc/self/stat").unwrap();
        let b = boot_time(&std::fs::read_to_string("/proc/stat").unwrap()).unwrap();
        let t = started(&me, b, unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64).unwrap();
        assert!(t >= b && t <= crate::now() + 1.0);
    }

    #[test]
    fn summaries() {
        let d = Doctor {
            checked: 8,
            missing: vec![
                (
                    "/usr/lib/openvpn3-linux/openvpn3-service-client".into(),
                    "libprotobuf.so.36.0.0".into(),
                ),
                ("/usr/bin/openvpn3".into(), "libprotobuf.so.36.0.0".into()),
            ],
            ..Default::default()
        };
        assert!(d.broken());
        assert_eq!(
            d.summary(),
            "openvpn3 cannot start: libprotobuf.so.36.0.0 is missing for openvpn3, openvpn3-service-client"
        );
        let ok = Doctor {
            checked: 8,
            ..Default::default()
        };
        assert_eq!(ok.summary(), "openvpn3 is fine (8 binaries link)");
    }
}
