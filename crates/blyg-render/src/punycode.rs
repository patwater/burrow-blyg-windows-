//! Punycode (RFC 3492) with the exact `toASCII` / `toUnicode` domain mapping
//! of punycode.js, which markdown-it uses when normalising link hosts.
//!
//! punycode.js does no IDNA validation or case mapping: `toASCII` encodes
//! every label that contains a non-ASCII code point, and `toUnicode` decodes
//! every label that starts with `xn--` (lower-casing the rest first). A label
//! that fails to decode makes the whole call fail, and markdown-it then keeps
//! the hostname unchanged; `None` models that.

const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(mut delta: u32, num_points: u32, first_time: bool) -> u32 {
    delta = if first_time { delta / DAMP } else { delta / 2 };
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (BASE - TMIN + 1) * delta / (delta + SKEW)
}

fn digit_to_basic(d: u32) -> char {
    // 0..25 -> a..z, 26..35 -> 0..9
    if d < 26 {
        (b'a' + d as u8) as char
    } else {
        (b'0' + (d - 26) as u8) as char
    }
}

fn basic_to_digit(c: u32) -> u32 {
    if (0x30..0x3A).contains(&c) {
        26 + (c - 0x30)
    } else if (0x41..0x5B).contains(&c) {
        c - 0x41
    } else if (0x61..0x7B).contains(&c) {
        c - 0x61
    } else {
        BASE
    }
}

/// Encode one label (no `xn--` prefix).
pub fn encode(input: &str) -> Option<String> {
    let cps: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut out = String::new();
    for &c in &cps {
        if c < 0x80 {
            out.push(char::from_u32(c)?);
        }
    }
    let basic_len = out.len() as u32;
    let mut handled = basic_len;
    if basic_len > 0 {
        out.push('-');
    }
    let mut n = INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = INITIAL_BIAS;
    let input_len = cps.len() as u32;
    while handled < input_len {
        let m = cps.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit_to_basic(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit_to_basic(q));
                bias = adapt(delta, handled + 1, handled == basic_len);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
}

/// Decode one label (no `xn--` prefix).
pub fn decode(input: &str) -> Option<String> {
    let bytes: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let basic = bytes.iter().rposition(|&c| c == 0x2D).unwrap_or(0);
    let mut output: Vec<u32> = Vec::new();
    for &c in &bytes[..basic] {
        if c >= 0x80 {
            return None;
        }
        output.push(c);
    }
    let mut i: u32 = 0;
    let mut n = INITIAL_N;
    let mut bias = INITIAL_BIAS;
    let mut idx = if basic > 0 { basic + 1 } else { 0 };
    while idx < bytes.len() {
        let old_i = i;
        let mut w: u32 = 1;
        let mut k = BASE;
        loop {
            if idx >= bytes.len() {
                return None;
            }
            let digit = basic_to_digit(bytes[idx]);
            idx += 1;
            if digit >= BASE {
                return None;
            }
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias {
                TMIN
            } else if k >= bias + TMAX {
                TMAX
            } else {
                k - bias
            };
            if digit < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let out_len = output.len() as u32 + 1;
        bias = adapt(i - old_i, out_len, old_i == 0);
        n = n.checked_add(i / out_len)?;
        i %= out_len;
        output.insert(i as usize, n);
        i += 1;
    }
    output.into_iter().map(char::from_u32).collect()
}

fn is_separator(c: char) -> bool {
    matches!(c, '.' | '\u{3002}' | '\u{FF0E}' | '\u{FF61}')
}

fn map_domain(domain: &str, f: impl Fn(&str) -> Option<String>) -> Option<String> {
    let (prefix, rest) = match domain.split_once('@') {
        // punycode.js: `parts = domain.split('@')`, keeps parts[0] and parts[1] only.
        Some((user, host)) => (format!("{user}@"), host.split('@').next().unwrap_or("")),
        None => (String::new(), domain),
    };
    let normalized: String = rest
        .chars()
        .map(|c| if is_separator(c) { '.' } else { c })
        .collect();
    let mut out = prefix;
    for (i, label) in normalized.split('.').enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&f(label)?);
    }
    Some(out)
}

/// punycode.js `toASCII`.
pub fn to_ascii(domain: &str) -> Option<String> {
    map_domain(domain, |label| {
        if label.is_ascii() {
            Some(label.to_string())
        } else {
            Some(format!("xn--{}", encode(label)?))
        }
    })
}

/// punycode.js `toUnicode`.
pub fn to_unicode(domain: &str) -> Option<String> {
    map_domain(domain, |label| {
        if let Some(rest) = label.strip_prefix("xn--") {
            decode(&rest.to_lowercase())
        } else {
            Some(label.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(encode("bücher").as_deref(), Some("bcher-kva"));
        assert_eq!(decode("bcher-kva").as_deref(), Some("bücher"));
        assert_eq!(
            to_ascii("münchen.example").as_deref(),
            Some("xn--mnchen-3ya.example")
        );
        assert_eq!(
            to_unicode("xn--mnchen-3ya.example").as_deref(),
            Some("münchen.example")
        );
        assert_eq!(
            to_unicode("plain.example").as_deref(),
            Some("plain.example")
        );
        assert_eq!(decode("!!!"), None);
    }
}
