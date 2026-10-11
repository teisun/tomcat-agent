use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use getrandom::fill as fill_random;
use rquickjs::function::Func;
use rquickjs::Object;
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::convert::TryInto;
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
        "__pi_crypto_aes_gcm_encrypt_native",
        Func::from(
            move |key_base64: String,
                  iv_base64: String,
                  plaintext_base64: String,
                  aad_base64: String|
                  -> rquickjs::Result<String> {
                let key = decode_base64_input("crypto aes-gcm key", &key_base64)
                    .map_err(js_runtime_error)?;
                let iv = decode_base64_input("crypto aes-gcm iv", &iv_base64)
                    .map_err(js_runtime_error)?;
                let plaintext = decode_base64_input("crypto aes-gcm plaintext", &plaintext_base64)
                    .map_err(js_runtime_error)?;
                let aad = decode_optional_base64_input("crypto aes-gcm aad", &aad_base64)
                    .map_err(js_runtime_error)?;
                let sealed =
                    aes_gcm_encrypt(&key, &iv, &plaintext, &aad).map_err(js_runtime_error)?;
                Ok(BASE64_STANDARD.encode(sealed))
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_aes_gcm_decrypt_native",
        Func::from(
            move |key_base64: String,
                  iv_base64: String,
                  sealed_base64: String,
                  aad_base64: String|
                  -> rquickjs::Result<String> {
                let key = decode_base64_input("crypto aes-gcm key", &key_base64)
                    .map_err(js_runtime_error)?;
                let iv = decode_base64_input("crypto aes-gcm iv", &iv_base64)
                    .map_err(js_runtime_error)?;
                let sealed = decode_base64_input("crypto aes-gcm ciphertext", &sealed_base64)
                    .map_err(js_runtime_error)?;
                let aad = decode_optional_base64_input("crypto aes-gcm aad", &aad_base64)
                    .map_err(js_runtime_error)?;
                let plaintext =
                    aes_gcm_decrypt(&key, &iv, &sealed, &aad).map_err(js_runtime_error)?;
                Ok(BASE64_STANDARD.encode(plaintext))
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_ed25519_generate_keypair_native",
        Func::from(move |seed_base64: String| -> rquickjs::Result<String> {
            let seed = if seed_base64.is_empty() {
                None
            } else {
                Some(
                    decode_base64_input("crypto ed25519 seed", &seed_base64)
                        .map_err(js_runtime_error)?,
                )
            };
            let (public_key, secret_key) =
                ed25519_generate_keypair(seed.as_deref()).map_err(js_runtime_error)?;
            Ok(serde_json::json!({
                "publicKey": BASE64_STANDARD.encode(public_key),
                "secretKey": BASE64_STANDARD.encode(secret_key),
            })
            .to_string())
        }),
    )?;
    globals.set(
        "__pi_crypto_ed25519_sign_native",
        Func::from(
            move |secret_key_base64: String, data_base64: String| -> rquickjs::Result<String> {
                let secret_key =
                    decode_base64_input("crypto ed25519 secret key", &secret_key_base64)
                        .map_err(js_runtime_error)?;
                let data = decode_base64_input("crypto ed25519 data", &data_base64)
                    .map_err(js_runtime_error)?;
                let signature = ed25519_sign(&secret_key, &data).map_err(js_runtime_error)?;
                Ok(BASE64_STANDARD.encode(signature))
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_ed25519_verify_native",
        Func::from(
            move |public_key_base64: String,
                  data_base64: String,
                  signature_base64: String|
                  -> rquickjs::Result<bool> {
                let public_key =
                    decode_base64_input("crypto ed25519 public key", &public_key_base64)
                        .map_err(js_runtime_error)?;
                let data = decode_base64_input("crypto ed25519 data", &data_base64)
                    .map_err(js_runtime_error)?;
                let signature = decode_base64_input("crypto ed25519 signature", &signature_base64)
                    .map_err(js_runtime_error)?;
                ed25519_verify(&public_key, &data, &signature).map_err(js_runtime_error)
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

pub(crate) fn aes_gcm_encrypt(
    key: &[u8],
    iv: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    if iv.len() != 12 {
        return Err(format!("aes-gcm iv must be 12 bytes, got {}", iv.len()));
    }

    let nonce = Nonce::from_slice(iv);
    match key.len() {
        16 => {
            let cipher = Aes128Gcm::new_from_slice(key)
                .map_err(|_| "failed to initialize aes-128-gcm".to_string())?;
            cipher
                .encrypt(
                    nonce,
                    Payload {
                        msg: plaintext,
                        aad,
                    },
                )
                .map_err(|_| "aes-gcm encrypt failed".to_string())
        }
        32 => {
            let cipher = Aes256Gcm::new_from_slice(key)
                .map_err(|_| "failed to initialize aes-256-gcm".to_string())?;
            cipher
                .encrypt(
                    nonce,
                    Payload {
                        msg: plaintext,
                        aad,
                    },
                )
                .map_err(|_| "aes-gcm encrypt failed".to_string())
        }
        len => Err(format!("aes-gcm key must be 16 or 32 bytes, got {len}")),
    }
}

pub(crate) fn aes_gcm_decrypt(
    key: &[u8],
    iv: &[u8],
    ciphertext_and_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    if iv.len() != 12 {
        return Err(format!("aes-gcm iv must be 12 bytes, got {}", iv.len()));
    }

    let nonce = Nonce::from_slice(iv);
    match key.len() {
        16 => {
            let cipher = Aes128Gcm::new_from_slice(key)
                .map_err(|_| "failed to initialize aes-128-gcm".to_string())?;
            cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: ciphertext_and_tag,
                        aad,
                    },
                )
                .map_err(|_| "aes-gcm decrypt failed".to_string())
        }
        32 => {
            let cipher = Aes256Gcm::new_from_slice(key)
                .map_err(|_| "failed to initialize aes-256-gcm".to_string())?;
            cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: ciphertext_and_tag,
                        aad,
                    },
                )
                .map_err(|_| "aes-gcm decrypt failed".to_string())
        }
        len => Err(format!("aes-gcm key must be 16 or 32 bytes, got {len}")),
    }
}

pub(crate) fn ed25519_generate_keypair(seed: Option<&[u8]>) -> Result<(Vec<u8>, Vec<u8>), String> {
    let secret_key = match seed {
        Some(seed_bytes) => seed_bytes.try_into().map_err(|_| {
            format!(
                "ed25519 seed must be exactly 32 bytes, got {}",
                seed_bytes.len()
            )
        })?,
        None => {
            let mut random_seed = [0_u8; 32];
            fill_random(&mut random_seed)
                .map_err(|error| format!("fill ed25519 seed failed: {error}"))?;
            random_seed
        }
    };

    let signing_key = SigningKey::from_bytes(&secret_key);
    let public_key = signing_key.verifying_key().to_bytes();
    Ok((public_key.to_vec(), signing_key.to_bytes().to_vec()))
}

pub(crate) fn ed25519_sign(secret_key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let secret_key: [u8; 32] = secret_key.try_into().map_err(|_| {
        format!(
            "ed25519 secret key must be exactly 32 bytes, got {}",
            secret_key.len()
        )
    })?;
    let signing_key = SigningKey::from_bytes(&secret_key);
    Ok(signing_key.sign(data).to_bytes().to_vec())
}

pub(crate) fn ed25519_verify(
    public_key: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<bool, String> {
    let public_key: [u8; 32] = public_key.try_into().map_err(|_| {
        format!(
            "ed25519 public key must be exactly 32 bytes, got {}",
            public_key.len()
        )
    })?;
    let signature: [u8; 64] = signature.try_into().map_err(|_| {
        format!(
            "ed25519 signature must be exactly 64 bytes, got {}",
            signature.len()
        )
    })?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|error| format!("invalid ed25519 public key: {error}"))?;
    Ok(verifying_key
        .verify(data, &Signature::from_bytes(&signature))
        .is_ok())
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

fn decode_base64_input(label: &str, input: &str) -> Result<Vec<u8>, String> {
    BASE64_STANDARD
        .decode(input)
        .map_err(|error| format!("invalid base64 input for {label}: {error}"))
}

fn decode_optional_base64_input(label: &str, input: &str) -> Result<Vec<u8>, String> {
    if input.is_empty() {
        Ok(Vec::new())
    } else {
        decode_base64_input(label, input)
    }
}

fn js_runtime_error(message: impl Into<String>) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("RustHost", "QuickJsHost", message.into())
}

#[cfg(test)]
#[path = "tests/crypto_native_test.rs"]
mod tests;
