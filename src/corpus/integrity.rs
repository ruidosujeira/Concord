//! Subresource-integrity verification of npm tarballs.
//!
//! The registry publishes `dist.integrity` as `<algorithm>-<base64 digest>`.
//! Only digests Concord can actually compute are accepted; anything else is an
//! acquisition failure rather than a silently unverified download.

use sha2::{Digest, Sha256, Sha512};

/// Verify `bytes` against one or more space-separated integrity values. One
/// matching value is enough, which is how the specification defines it.
pub fn verify(bytes: &[u8], integrity: &str) -> std::result::Result<(), String> {
    let mut attempted = Vec::new();
    for value in integrity.split_whitespace() {
        let Some((algorithm, encoded)) = value.split_once('-') else {
            continue;
        };
        let actual = match algorithm {
            "sha512" => Sha512::digest(bytes).to_vec(),
            "sha256" => Sha256::digest(bytes).to_vec(),
            _ => {
                attempted.push(algorithm.to_owned());
                continue;
            }
        };
        let expected = decode_base64(encoded)
            .ok_or_else(|| format!("integrity value is not valid base64: {value}"))?;
        if expected == actual {
            return Ok(());
        }
        return Err(format!(
            "tarball integrity mismatch\nexpected: {value}\nactual:   {algorithm}-{}",
            encode_base64(&actual)
        ));
    }
    if attempted.is_empty() {
        Err("the registry did not publish a usable dist.integrity value".to_owned())
    } else {
        Err(format!(
            "unsupported integrity algorithm(s): {}",
            attempted.join(", ")
        ))
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut collected = 0u32;
    let mut padding = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        let value = if *byte == b'=' {
            if index + 2 < bytes.len() {
                return None;
            }
            padding += 1;
            0
        } else {
            if padding > 0 {
                return None;
            }
            ALPHABET.iter().position(|entry| entry == byte)? as u32
        };
        accumulator = (accumulator << 6) | value;
        collected += 1;
        if collected == 4 {
            output.push((accumulator >> 16) as u8);
            output.push((accumulator >> 8) as u8);
            output.push(accumulator as u8);
            accumulator = 0;
            collected = 0;
        }
    }
    output.truncate(output.len() - padding);
    Some(output)
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value =
            (u32::from(buffer[0]) << 16) | (u32::from(buffer[1]) << 8) | u32::from(buffer[2]);
        for offset in 0..4 {
            if offset <= chunk.len() {
                let index = ((value >> (18 - offset * 6)) & 0x3f) as usize;
                output.push(char::from(ALPHABET[index]));
            } else {
                output.push('=');
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha512};

    use super::{decode_base64, encode_base64, verify};

    fn integrity(bytes: &[u8]) -> String {
        format!("sha512-{}", encode_base64(&Sha512::digest(bytes)))
    }

    #[test]
    fn base64_round_trips_every_padding_length() {
        for payload in [
            &b""[..],
            &b"a"[..],
            &b"ab"[..],
            &b"abc"[..],
            &b"abcd"[..],
            &b"\x00\xff\x10 binary \xfe"[..],
        ] {
            let encoded = encode_base64(payload);
            assert_eq!(
                decode_base64(&encoded).as_deref(),
                Some(payload),
                "round trip failed for {payload:?} ({encoded})"
            );
        }
    }

    #[test]
    fn a_matching_digest_verifies() {
        let bytes = b"tarball contents";
        assert_eq!(verify(bytes, &integrity(bytes)), Ok(()));
    }

    #[test]
    fn a_mismatched_digest_is_a_failure() {
        let error = verify(b"tampered", &integrity(b"original")).expect_err("mismatch");
        assert!(error.contains("integrity mismatch"), "{error}");
    }

    #[test]
    fn an_unusable_or_absent_integrity_value_is_a_failure() {
        assert!(verify(b"x", "").expect_err("absent").contains("usable"));
        assert!(
            verify(b"x", "sha1-abcdef")
                .expect_err("unsupported")
                .contains("unsupported integrity algorithm")
        );
        assert!(
            verify(b"x", "sha512-not base64!")
                .expect_err("invalid")
                .contains("base64")
        );
    }

    #[test]
    fn one_matching_value_among_several_is_enough() {
        let bytes = b"tarball contents";
        let value = format!("sha1-ignored {}", integrity(bytes));
        assert_eq!(verify(bytes, &value), Ok(()));
    }
}
