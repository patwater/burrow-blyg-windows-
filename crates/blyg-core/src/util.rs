//! Small helpers: wall-clock time, ISO-8601 timestamps, local ids.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::LocalId;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `2026-09-24T12:34:56.789Z`, same shape as the Worker's `nowIso()` so the two
/// sort together lexicographically.
pub fn now_iso() -> String {
    iso_from_ms(now_ms())
}

pub fn iso_from_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Parse a timestamp as feeds and blygs write them, to unix ms (UTC):
/// ISO 8601 / RFC 3339 (`2026-09-24T12:34:56.789Z`, `+02:00` offsets, a space
/// for the `T`, a bare date) and RFC 2822 (`Tue, 22 Sep 2026 08:00:00 GMT`,
/// RSS `pubDate`). `None` when it isn't one of those.
pub fn parse_ts_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() || !s.is_ascii() {
        return None;
    }
    parse_iso_ms(s).or_else(|| parse_rfc2822_ms(s))
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn civil_ms(y: i64, m: i64, d: i64, hms: (i64, i64, i64), ms: i64) -> Option<i64> {
    let (hh, mm, ss) = hms;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    Some(((days * 86_400) + hh * 3600 + mm * 60 + ss) * 1000 + ms)
}

fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (num(&s[0..4])?, num(&s[5..7])?, num(&s[8..10])?);
    let rest = &s[10..];
    if rest.is_empty() {
        return civil_ms(y, m, d, (0, 0, 0), 0);
    }
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    if rest.len() < 5 || rest.as_bytes()[2] != b':' {
        return None;
    }
    let (hh, mm) = (num(&rest[0..2])?, num(&rest[3..5])?);
    let mut rest = &rest[5..];
    let mut ss = 0;
    let mut ms = 0;
    if let Some(r) = rest.strip_prefix(':') {
        ss = num(r.get(0..2)?)?;
        rest = &r[2..];
        if let Some(r) = rest.strip_prefix(['.', ',']) {
            let digits = r.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let frac = &r[..digits.min(3)];
            ms = num(frac)? * 10_i64.pow(3 - frac.len() as u32);
            rest = &r[digits..];
        }
    }
    let offset_min = match rest {
        "" | "Z" | "z" => 0,
        o => {
            let sign = match o.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let o = o[1..].replace(':', "");
            if o.len() != 4 {
                return None;
            }
            sign * (num(&o[0..2])? * 60 + num(&o[2..4])?)
        }
    };
    Some(civil_ms(y, m, d, (hh, mm, ss), ms)? - offset_min * 60_000)
}

fn parse_rfc2822_ms(s: &str) -> Option<i64> {
    // Drop an optional weekday ("Tue,").
    let s = match s.find(',') {
        Some(i) => &s[i + 1..],
        None => s,
    };
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let d = num(parts[0])?;
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mon = parts[1].to_ascii_lowercase();
    let m = MONTHS.iter().position(|x| mon.starts_with(x))? as i64 + 1;
    let mut y = num(parts[2])?;
    if parts[2].len() == 2 {
        y += if y < 50 { 2000 } else { 1900 };
    }
    let t: Vec<&str> = parts[3].split(':').collect();
    if t.len() < 2 {
        return None;
    }
    let (hh, mm) = (num(t[0])?, num(t[1])?);
    let ss = match t.get(2) {
        Some(x) => num(x)?,
        None => 0,
    };
    let offset_min = match parts.get(4).copied().unwrap_or("GMT") {
        "EST" | "CDT" => -300,
        "EDT" => -240,
        "CST" | "MDT" => -360,
        "MST" | "PDT" => -420,
        "PST" => -480,
        o if o.len() == 5 && (o.starts_with('+') || o.starts_with('-')) => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (num(&o[1..3])? * 60 + num(&o[3..5])?)
        }
        // GMT, UT, UTC, Z, and zones we don't know.
        _ => 0,
    };
    Some(civil_ms(y, m, d, (hh, mm, ss), 0)? - offset_min * 60_000)
}

/// Howard Hinnant's civil date → days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's days → civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique per process and practically unique across runs: time + pid + counter.
pub fn new_local_id() -> LocalId {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    LocalId(format!(
        "L{:x}{:05x}{:04x}",
        nanos,
        std::process::id() & 0xfffff,
        n & 0xffff
    ))
}

/// SHA-256 of `data`, used to check a served `content_hash`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_format() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(1_790_000_000_123), "2026-09-21T14:13:20.123Z");
        assert_eq!(iso_from_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn timestamps_parse() {
        let ms = |s| parse_ts_ms(s).map(iso_from_ms);
        let z = "2026-09-21T14:13:20.000Z";
        assert_eq!(
            ms("2026-09-21T14:13:20.123Z").as_deref(),
            Some("2026-09-21T14:13:20.123Z")
        );
        assert_eq!(ms("2026-09-21T16:13:20+02:00").as_deref(), Some(z));
        assert_eq!(
            ms("2026-09-21T10:13:20.5-0400").as_deref(),
            Some("2026-09-21T14:13:20.500Z")
        );
        assert_eq!(ms("2026-09-21 14:13:20").as_deref(), Some(z));
        assert_eq!(
            ms("2026-09-21T14:13Z").as_deref(),
            Some("2026-09-21T14:13:00.000Z")
        );
        assert_eq!(
            ms("2026-09-21").as_deref(),
            Some("2026-09-21T00:00:00.000Z")
        );
        assert_eq!(
            ms("2026-09-21T14:13:20.123456789Z").as_deref(),
            Some("2026-09-21T14:13:20.123Z")
        );
        assert_eq!(ms("Mon, 21 Sep 2026 14:13:20 GMT").as_deref(), Some(z));
        assert_eq!(ms("21 Sep 2026 16:13:20 +0200").as_deref(), Some(z));
        assert_eq!(
            ms("2000-02-29T00:00:00Z").as_deref(),
            Some("2000-02-29T00:00:00.000Z")
        );
        for bad in [
            "",
            "garbage",
            "2026-13-01",
            "2026-09-21T25:00:00Z",
            "2026-09-21X",
            "Mon, 21 Foo 2026 14:13:20",
            "2026-09-21T14:13:20+2",
        ] {
            assert_eq!(parse_ts_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn sha256_known_answers() {
        let h = |s: &[u8]| hex(&sha256(s));
        assert_eq!(
            h(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            h(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            h(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            h(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // 64 bytes: the padding spills into a second block
        assert_eq!(
            h(&[b'x'; 64]),
            "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c"
        );
    }

    #[test]
    fn ids_unique() {
        let a = new_local_id();
        let b = new_local_id();
        assert_ne!(a, b);
    }
}
