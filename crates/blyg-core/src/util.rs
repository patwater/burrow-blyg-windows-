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
