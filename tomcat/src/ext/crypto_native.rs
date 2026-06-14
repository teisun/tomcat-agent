use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use getrandom::fill as fill_random;
use rquickjs::function::Func;
use rquickjs::Object;
use sha2::{Digest, Sha256, Sha384, Sha512};
use uuid::Uuid;

enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    fn parse(algo: &str) -> Result<Self, String> {
        match algo.trim().to_ascii_lowercase().as_str() {
            "sha256" => Ok(Self::Sha256),
            "sha384" => Ok(Self::Sha384),
            "sha512" => Ok(Self::Sha512),
            unsupported => Err(format!(
                "unsupported hash algorithm '{unsupported}', expected sha256/sha384/sha512"
            )),
        }
    }

    fn block_size(&self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha384 | Self::Sha512 => 128,
        }
    }

    fn digest(&self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha256 => Sha256::digest(bytes).to_vec(),
            Self::Sha384 => Sha384::digest(bytes).to_vec(),
            Self::Sha512 => Sha512::digest(bytes).to_vec(),
        }
    }
}

pub(crate) fn register_crypto_globals<'js>(globals: &Object<'js>) -> rquickjs::Result<()> {
    globals.set(
        "__pi_crypto_hash_native",
        Func::from(
            move |algo: String, data_base64: String| -> rquickjs::Result<String> {
                let data = BASE64_STANDARD.decode(data_base64).map_err(|error| {
                    js_runtime_error(format!("invalid base64 input for crypto hash: {error}"))
                })?;
                hash_bytes_hex(&algo, &data).map_err(js_runtime_error)
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_hmac_native",
        Func::from(
            move |algo: String,
                  key_base64: String,
                  data_base64: String|
                  -> rquickjs::Result<String> {
                let key = BASE64_STANDARD.decode(key_base64).map_err(|error| {
                    js_runtime_error(format!("invalid base64 input for crypto hmac key: {error}"))
                })?;
                let data = BASE64_STANDARD.decode(data_base64).map_err(|error| {
                    js_runtime_error(format!(
                        "invalid base64 input for crypto hmac data: {error}"
                    ))
                })?;
                hmac_bytes_hex(&algo, &key, &data).map_err(js_runtime_error)
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_random_uuid_native",
        Func::from(move || -> rquickjs::Result<String> { Ok(random_uuid_v4()) }),
    )?;
    globals.set(
        "__pi_crypto_random_bytes_native",
        Func::from(move |size: u32| -> rquickjs::Result<String> {
            random_bytes_hex(size as usize).map_err(js_runtime_error)
        }),
    )?;
    Ok(())
}

pub(crate) fn hash_bytes_hex(algo: &str, bytes: &[u8]) -> Result<String, String> {
    let algorithm = HashAlgorithm::parse(algo)?;
    Ok(bytes_to_hex(&algorithm.digest(bytes)))
}

pub(crate) fn hmac_bytes_hex(algo: &str, key: &[u8], bytes: &[u8]) -> Result<String, String> {
    let algorithm = HashAlgorithm::parse(algo)?;
    let block_size = algorithm.block_size();
    let mut normalized_key = if key.len() > block_size {
        algorithm.digest(key)
    } else {
        key.to_vec()
    };
    normalized_key.resize(block_size, 0);

    let mut inner_pad = vec![0x36; block_size];
    let mut outer_pad = vec![0x5c; block_size];
    for (index, byte) in normalized_key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }

    let mut inner = Vec::with_capacity(block_size + bytes.len());
    inner.extend_from_slice(&inner_pad);
    inner.extend_from_slice(bytes);
    let inner_digest = algorithm.digest(&inner);

    let mut outer = Vec::with_capacity(block_size + inner_digest.len());
    outer.extend_from_slice(&outer_pad);
    outer.extend_from_slice(&inner_digest);
    Ok(bytes_to_hex(&algorithm.digest(&outer)))
}

pub(crate) fn random_uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn random_bytes_hex(size: usize) -> Result<String, String> {
    if size > 10 * 1024 * 1024 {
        return Err(format!("randomBytes size exceeds limit: {size}"));
    }
    let mut bytes = vec![0_u8; size];
    fill_random(&mut bytes).map_err(|error| format!("fill random bytes failed: {error}"))?;
    Ok(bytes_to_hex(&bytes))
}

pub(crate) fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn js_runtime_error(message: impl Into<String>) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("RustHost", "QuickJsHost", message.into())
}

#[cfg(test)]
mod tests {
    use super::{hmac_bytes_hex, random_uuid_v4};
    use crate::ext::crypto_native::hash_bytes_hex;

    #[test]
    fn sha256_known_answer_matches() {
        let digest = hash_bytes_hex("sha256", b"abc").expect("sha256 kat");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_sha256_known_answer_matches() {
        let digest = hmac_bytes_hex(
            "sha256",
            b"key",
            b"The quick brown fox jumps over the lazy dog",
        )
        .expect("hmac kat");
        assert_eq!(
            digest,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn random_uuid_v4_has_expected_shape() {
        let uuid = random_uuid_v4();
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(
            matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "uuid variant should be RFC4122-compatible: {uuid}"
        );
    }
}
