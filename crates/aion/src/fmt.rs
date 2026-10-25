//! Number, time and duration formatting shared by the views and the chat summary.

use chrono::{Local, TimeZone};

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `950`, `23.4K`, `1.25M`, `2.10B`.
pub fn format_short(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "0".into();
    }
    let units = [(1e9, "B"), (1e6, "M"), (1e3, "K")];
    for (scale, suffix) in units {
        if value >= scale {
            let scaled = value / scale;
            let digits = if scaled >= 100.0 {
                0
            } else if scaled >= 10.0 {
                1
            } else {
                2
            };
            return format!("{scaled:.digits$}{suffix}");
        }
    }
    format!("{}", value.round() as u64)
}

/// `format_short` with a sign: `+1.51M`, `-35.0K`, `0`.
pub fn format_signed(value: f64) -> String {
    if !value.is_finite() || value.round() == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "+" };
    format!("{sign}{}", format_short(value.abs()))
}

/// `8,308,193`.
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `+1,511,238`, `-35,000`, `0`.
pub fn group_signed(n: i64) -> String {
    match n {
        0 => "0".into(),
        n if n < 0 => format!("-{}", group(n.unsigned_abs())),
        n => format!("+{}", group(n as u64)),
    }
}

/// Local wall clock `14:05`.
pub fn clock(t_ms: u64) -> String {
    local(t_ms).map(|d| d.format("%H:%M").to_string()).unwrap_or_else(|| "--:--".into())
}

/// Local wall clock with seconds, `14:05:09`.
pub fn clock_secs(t_ms: u64) -> String {
    local(t_ms).map(|d| d.format("%H:%M:%S").to_string()).unwrap_or_else(|| "--:--:--".into())
}

/// `Today`, `Yesterday` or a short date such as `Sat 11 Oct`.
pub fn day(t_ms: u64, now_ms: u64) -> String {
    let (Some(d), Some(now)) = (local(t_ms), local(now_ms)) else { return "?".into() };
    let days = (now.date_naive() - d.date_naive()).num_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if days < 300 => d.format("%a %e %b").to_string(),
        _ => d.format("%e %b %Y").to_string(),
    }
}

/// Whether `t_ms` falls on the same local day as `now_ms`.
pub fn same_day(t_ms: u64, now_ms: u64) -> bool {
    matches!((local(t_ms), local(now_ms)), (Some(a), Some(b)) if a.date_naive() == b.date_naive())
}

fn local(t_ms: u64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_millis_opt(i64::try_from(t_ms).ok()?).single()
}

/// `5000000`, `5m`, `750k`, `1.5m`, `2b`, `5,000,000`.
pub fn parse_amount(text: &str) -> Option<u64> {
    let t = text.trim().to_ascii_lowercase().replace([',', '_'], "");
    let (num, scale) = match t.chars().last()? {
        'k' => (&t[..t.len() - 1], 1e3),
        'm' => (&t[..t.len() - 1], 1e6),
        'b' => (&t[..t.len() - 1], 1e9),
        _ => (t.as_str(), 1.0),
    };
    let value: f64 = num.parse().ok()?;
    (value.is_finite() && value >= 0.0).then(|| (value * scale).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_take_suffixes() {
        assert_eq!(parse_amount("5m"), Some(5_000_000));
        assert_eq!(parse_amount("750k"), Some(750_000));
        assert_eq!(parse_amount("1.5M"), Some(1_500_000));
        assert_eq!(parse_amount("5,000,000"), Some(5_000_000));
        assert_eq!(parse_amount("-5m"), None);
        assert_eq!(parse_amount(""), None);
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_short(950.0), "950");
        assert_eq!(format_short(23_400.0), "23.4K");
        assert_eq!(format_short(1_250_000.0), "1.25M");
        assert_eq!(format_short(756_946.0), "757K");
        assert_eq!(format_short(-5.0), "0");
        assert_eq!(format_signed(-35_000.0), "-35.0K");
        assert_eq!(format_signed(1_511_238.0), "+1.51M");
        assert_eq!(format_signed(0.2), "0");
        assert_eq!(format_duration(59_000), "0:59");
        assert_eq!(format_duration(7_187_000), "1:59:47");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(8_308_193), "8,308,193");
        assert_eq!(group_signed(-35_000), "-35,000");
        assert_eq!(group_signed(1_511_238), "+1,511,238");
        assert_eq!(group_signed(0), "0");
    }

    #[test]
    fn days_are_relative() {
        let now = 1_791_722_000_000;
        assert_eq!(day(now, now), "Today");
        assert_eq!(day(now - 86_400_000, now), "Yesterday");
        assert!(same_day(now, now));
        assert!(!same_day(now - 3 * 86_400_000, now));
        assert_eq!(clock(now).len(), 5);
    }
}
