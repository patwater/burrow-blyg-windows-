//! Signature and checksum checks for a downloaded release.
//!
//! The release workflow signs `SHA256SUMS` with the project's Ed25519 key and
//! publishes the raw 64-byte signature as `SHA256SUMS.sig`. The app embeds
//! the public key and refuses a release unless the signature verifies
//! (`verify_strict`) and the zip's SHA-256 matches its line in the signed
//! `SHA256SUMS`.

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use super::UpdateError;

/// The release signing key (raw 32-byte Ed25519 public key, base64). Its
/// private half is the `UPDATE_SIGNING_KEY` secret of the release workflow.
pub const RELEASE_PUBLIC_KEY_B64: &str = "mnXJcOWPGSTrMKx38w6FqoKQxky6+pj3Ch2IVPrk3+I=";

/// The embedded release key.
pub fn release_key() -> VerifyingKey {
    parse_public_key(RELEASE_PUBLIC_KEY_B64).expect("the embedded release key is valid")
}

pub fn parse_public_key(b64: &str) -> Result<VerifyingKey, UpdateError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| UpdateError::Verify("the public key isn't base64".into()))?;
    let raw: [u8; 32] = raw
        .try_into()
        .map_err(|_| UpdateError::Verify("the public key isn't 32 bytes".into()))?;
    VerifyingKey::from_bytes(&raw)
        .map_err(|_| UpdateError::Verify("the public key isn't a valid Ed25519 key".into()))
}

/// Check `sig` (the raw 64 bytes of `SHA256SUMS.sig`) over `sums` (the bytes
/// of `SHA256SUMS`, exactly as downloaded).
pub fn verify_sums_signature(
    key: &VerifyingKey,
    sums: &[u8],
    sig: &[u8],
) -> Result<(), UpdateError> {
    let sig: [u8; 64] = sig.try_into().map_err(|_| {
        UpdateError::Verify(format!(
            "SHA256SUMS.sig is {} bytes, not a 64-byte signature",
            sig.len()
        ))
    })?;
    key.verify_strict(sums, &Signature::from_bytes(&sig))
        .map_err(|_| UpdateError::Verify("the SHA256SUMS signature doesn't verify".into()))
}

/// The hex SHA-256 listed for `name` in a `shasum -a 256` file
/// (`<64 hex>  <name>`, or `<hex> *<name>` in binary mode).
pub fn expected_hash(sums: &str, name: &str) -> Result<[u8; 32], UpdateError> {
    let mut found = None;
    for line in sums.lines() {
        let Some((hash, file)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        if file != name {
            continue;
        }
        if found.is_some() {
            return Err(UpdateError::Verify(format!(
                "SHA256SUMS lists {name} more than once"
            )));
        }
        found = Some(parse_hex32(hash).ok_or_else(|| {
            UpdateError::Verify(format!("SHA256SUMS has a malformed hash for {name}"))
        })?);
    }
    found.ok_or_else(|| UpdateError::Verify(format!("SHA256SUMS doesn't list {name}")))
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The zip matches its line in the (already verified) `SHA256SUMS`.
pub fn verify_hash(sums: &str, name: &str, bytes: &[u8]) -> Result<(), UpdateError> {
    let want = expected_hash(sums, name)?;
    if sha256(bytes) != want {
        return Err(UpdateError::Verify(format!(
            "{name} doesn't match its SHA-256 in SHA256SUMS"
        )));
    }
    Ok(())
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.as_bytes();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn the_embedded_key_parses() {
        assert_eq!(release_key().to_bytes().len(), 32);
        assert!(parse_public_key("bm90IGEga2V5").is_err(), "too short");
        assert!(parse_public_key("not base64!").is_err());
    }

    #[test]
    fn install_sh_trusts_the_same_key() {
        let script = include_str!("../../../../scripts/install.sh");
        assert!(
            script.contains(&format!("UPDATE_PUBKEY=\"{RELEASE_PUBLIC_KEY_B64}\"")),
            "scripts/install.sh's UPDATE_PUBKEY must match RELEASE_PUBLIC_KEY_B64"
        );
    }

    #[test]
    fn signatures_verify_only_with_the_right_key_and_bytes() {
        let sk = test_key();
        let sums = b"abc  Blygger-9.9.9-macos-universal.zip\n";
        let sig = sk.sign(sums).to_bytes();
        let pk = sk.verifying_key();
        verify_sums_signature(&pk, sums, &sig).unwrap();
        // Tampered file.
        assert!(verify_sums_signature(&pk, b"abd  Blygger.zip\n", &sig).is_err());
        // Tampered signature.
        let mut bad = sig;
        bad[3] ^= 1;
        assert!(verify_sums_signature(&pk, sums, &bad).is_err());
        // Another key (e.g. the real release key) doesn't accept it.
        assert!(verify_sums_signature(&release_key(), sums, &sig).is_err());
        // Wrong length (e.g. base64 text instead of raw bytes).
        assert!(verify_sums_signature(&pk, sums, &sig[..63]).is_err());
        let b64 = base64::engine::general_purpose::STANDARD.encode(sig);
        assert!(verify_sums_signature(&pk, sums, b64.as_bytes()).is_err());
    }

    /// What the release workflow produces: `openssl pkeyutl -sign -rawin`
    /// (OpenSSL 3) with a throwaway key, over a SHA256SUMS line.
    #[test]
    fn an_openssl_signature_verifies() {
        let pk = parse_public_key("zhErrAPnRearAri+EAHnlwqYO2+AdvfdufPBM1IwvsA=").unwrap();
        let sig = base64::engine::general_purpose::STANDARD
            .decode(
                "Kb+GdWoKrZp3o1G9V3rCZavuGtFIPGFvGpJW2iiUcb9MFvq/kOWbwYNxKepQfLXvRt38DyKMZRJwW3cieK+oAA==",
            )
            .unwrap();
        let sums = b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  \
                     Blygger-9.9.9-macos-universal.zip\n";
        verify_sums_signature(&pk, sums, &sig).unwrap();
        assert!(verify_sums_signature(&pk, &sums[1..], &sig).is_err());
    }

    #[test]
    fn hashes_are_looked_up_by_exact_name() {
        let zip = b"zip bytes";
        let h = hex(&sha256(zip));
        let sums = format!(
            "{h}  Blygger-1.0.0-macos-universal.zip\n{}  Blygger-1.0.0-macos-universal.dmg\n",
            hex(&sha256(b"dmg"))
        );
        verify_hash(&sums, "Blygger-1.0.0-macos-universal.zip", zip).unwrap();
        assert!(verify_hash(&sums, "Blygger-1.0.0-macos-universal.zip", b"other").is_err());
        assert!(
            verify_hash(&sums, "universal.zip", zip).is_err(),
            "no suffix match"
        );
        assert!(verify_hash(&sums, "Blygger-macos-universal.zip", zip).is_err());
        // Binary-mode marker.
        let bin = format!("{h} *a.zip\n");
        verify_hash(&bin, "a.zip", zip).unwrap();
        // Duplicate or malformed lines are refused.
        let dup = format!("{h}  a.zip\n{h}  a.zip\n");
        assert!(verify_hash(&dup, "a.zip", zip).is_err());
        assert!(verify_hash("xyz  a.zip\n", "a.zip", zip).is_err());
    }
}
