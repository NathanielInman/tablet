//! Text for the bar, the drawer and the CLI.

/// Escapes text for Pango markup (waybar renders module text and tooltips
/// as markup).
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A length of time, short: `41s`, `3m`, `2h 14m`, `3d 4h`.
pub fn span(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => {
            let m = (s % 3600) / 60;
            if m == 0 {
                format!("{}h", s / 3600)
            } else {
                format!("{}h {m}m", s / 3600)
            }
        }
        _ => {
            let h = (s % 86_400) / 3600;
            if h == 0 {
                format!("{}d", s / 86_400)
            } else {
                format!("{}d {h}h", s / 86_400)
            }
        }
    }
}

/// How long ago, for "checked 40s ago": `just now` under five seconds.
pub fn ago(secs: f64) -> String {
    if secs < 5.0 {
        "just now".into()
    } else {
        format!("{} ago", span(secs))
    }
}

/// Bytes, short: `812 B`, `14.2 kB`, `3.1 MB`, `1.2 GB` (powers of 1000).
pub fn bytes(n: f64) -> String {
    let n = n.max(0.0);
    if n < 1000.0 {
        return format!("{n:.0} B");
    }
    let units = ["kB", "MB", "GB", "TB"];
    let mut v = n / 1000.0;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{v:.0} {}", units[i])
    }
}

/// A rate in bytes a second: `14.2 kB/s`.
pub fn rate(n: f64) -> String {
    format!("{}/s", bytes(n))
}

/// Local wall-clock `HH:MM` for Unix seconds, from the C library so the
/// user's zone and its DST rules apply.
pub fn hhmm(t: f64) -> String {
    let (h, m, _) = local(t);
    format!("{h:02}:{m:02}")
}

/// `HH:MM` today, `yesterday HH:MM`, or `Nov 10 HH:MM`.
pub fn when(t: f64, now: f64) -> String {
    let (_, _, day) = local(t);
    let (_, _, today) = local(now);
    if day == today {
        hhmm(t)
    } else if day + 1 == today {
        format!("yesterday {}", hhmm(t))
    } else {
        format!("{} {}", date(t), hhmm(t))
    }
}

/// Short, for a column: `14:32` today, `Thu 18:20` this week, `Nov 11`
/// before that.
pub fn short_when(t: f64, now: f64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let (_, _, day) = local(t);
    let (_, _, today) = local(now);
    if day == today {
        hhmm(t)
    } else if today - day < 7 && t <= now {
        format!("{} {}", DAYS[tm(t).tm_wday.clamp(0, 6) as usize], hhmm(t))
    } else {
        date(t)
    }
}

/// `Nov 10`.
pub fn date(t: f64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let tm = tm(t);
    format!(
        "{} {}",
        MONTHS[(tm.tm_mon.clamp(0, 11)) as usize],
        tm.tm_mday
    )
}

fn tm(t: f64) -> libc::tm {
    let secs = t as libc::time_t;
    let mut out: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut out);
    }
    out
}

/// Hour, minute and a day number that grows by one each local day.
fn local(t: f64) -> (i32, i32, i64) {
    let tm = tm(t);
    let day = (tm.tm_year as i64) * 400 + tm.tm_yday as i64;
    (tm.tm_hour, tm.tm_min, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        assert_eq!(span(41.0), "41s");
        assert_eq!(span(180.0), "3m");
        assert_eq!(span(3600.0), "1h");
        assert_eq!(span(2.0 * 3600.0 + 14.0 * 60.0 + 9.0), "2h 14m");
        assert_eq!(span(26.0 * 3600.0), "1d 2h");
        assert_eq!(span(48.0 * 3600.0), "2d");
        assert_eq!(span(-3.0), "0s");
        assert_eq!(ago(2.0), "just now");
        assert_eq!(ago(40.0), "40s ago");
    }

    #[test]
    fn sizes() {
        assert_eq!(bytes(812.0), "812 B");
        assert_eq!(bytes(14_200.0), "14 kB");
        assert_eq!(bytes(4_200.0), "4.2 kB");
        assert_eq!(bytes(1_522_947.0), "1.5 MB");
        assert_eq!(bytes(312_000_000.0), "312 MB");
        assert_eq!(rate(84_000.0), "84 kB/s");
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a & <b>"), "a &amp; &lt;b&gt;");
    }
}
