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
mod tests {
    use super::{
        aes_gcm_decrypt, aes_gcm_encrypt, bytes_to_hex, ed25519_generate_keypair, ed25519_sign,
        ed25519_verify, hash_bytes_hex, hmac_bytes_hex, random_uuid_v4,
    };

    #[test]
    fn sha256_known_answer_matches() {
        let digest = hash_bytes_hex("sha256", b"abc").expect("sha256 kat");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha384_sha512_known_answers() {
        let sha384 = hash_bytes_hex("sha384", b"abc").expect("sha384 kat");
        assert_eq!(
            sha384,
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7"
        );

        let sha512 = hash_bytes_hex("sha512", b"abc").expect("sha512 kat");
        assert_eq!(
            sha512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
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
    fn aes_gcm_known_answer_matches() {
        let key = decode_hex("00000000000000000000000000000000");
        let iv = decode_hex("000000000000000000000000");
        let plaintext = decode_hex("00000000000000000000000000000000");
        let sealed = aes_gcm_encrypt(&key, &iv, &plaintext, b"").expect("aes-gcm encrypt kat");
        assert_eq!(
            bytes_to_hex(&sealed),
            "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf"
        );

        let reopened = aes_gcm_decrypt(&key, &iv, &sealed, b"").expect("aes-gcm decrypt kat");
        assert_eq!(reopened, plaintext);
    }

    #[test]
    fn aes256_gcm_known_answer_roundtrip() {
        let key = decode_hex("0000000000000000000000000000000000000000000000000000000000000000");
        let iv = decode_hex("000000000000000000000000");
        let plaintext = decode_hex("00000000000000000000000000000000");
        let sealed = aes_gcm_encrypt(&key, &iv, &plaintext, b"").expect("aes-256-gcm encrypt kat");
        assert_eq!(
            bytes_to_hex(&sealed),
            "cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919"
        );

        let reopened = aes_gcm_decrypt(&key, &iv, &sealed, b"").expect("aes-256-gcm decrypt kat");
        assert_eq!(reopened, plaintext);
    }

    #[test]
    fn aes_gcm_roundtrip_with_aad() {
        let key = decode_hex("feffe9928665731c6d6a8f9467308308");
        let iv = decode_hex("cafebabefacedbaddecaf888");
        let plaintext = b"plugin-payload";
        let aad = b"session-42";

        let sealed = aes_gcm_encrypt(&key, &iv, plaintext, aad).expect("aes-gcm encrypt with aad");
        let reopened =
            aes_gcm_decrypt(&key, &iv, &sealed, aad).expect("aes-gcm decrypt with matching aad");
        assert_eq!(reopened, plaintext);
        assert!(
            aes_gcm_decrypt(&key, &iv, &sealed, b"wrong-aad").is_err(),
            "GCM should bind AAD into the authentication tag"
        );
    }

    #[test]
    fn aes_gcm_rejects_bad_iv_and_key_length() {
        let err = aes_gcm_encrypt(&[0_u8; 16], &[0_u8; 11], b"abc", b"")
            .expect_err("iv shorter than 12 bytes should fail");
        assert!(
            err.contains("iv must be 12 bytes"),
            "unexpected bad-iv error: {err}"
        );

        let err = aes_gcm_encrypt(&[0_u8; 24], &[0_u8; 12], b"abc", b"")
            .expect_err("unsupported key width should fail");
        assert!(
            err.contains("key must be 16 or 32 bytes"),
            "unexpected bad-key error: {err}"
        );
    }

    #[test]
    fn ed25519_rfc8032_vector_matches() {
        let secret_key =
            decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let expected_public_key =
            decode_hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let expected_signature = decode_hex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
             5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );

        let (public_key, generated_secret_key) =
            ed25519_generate_keypair(Some(&secret_key)).expect("ed25519 generate keypair");
        assert_eq!(public_key, expected_public_key);
        assert_eq!(generated_secret_key, secret_key);

        let signature = ed25519_sign(&generated_secret_key, b"").expect("ed25519 sign");
        assert_eq!(signature, expected_signature);
        assert!(
            ed25519_verify(&public_key, b"", &signature).expect("ed25519 verify"),
            "signature should verify against RFC8032 vector"
        );
    }

    #[test]
    fn ed25519_generate_keypair_without_seed_succeeds() {
        let (public_key, secret_key) =
            ed25519_generate_keypair(None).expect("generate random ed25519 keypair");
        assert_eq!(
            public_key.len(),
            32,
            "ed25519 public key should be 32 bytes"
        );
        assert_eq!(
            secret_key.len(),
            32,
            "ed25519 secret key should be 32 bytes"
        );

        let signature = ed25519_sign(&secret_key, b"plugin-signature").expect("sign test payload");
        assert!(
            ed25519_verify(&public_key, b"plugin-signature", &signature)
                .expect("verify random keypair signature"),
            "freshly generated keypair should self-verify"
        );
    }

    #[test]
    fn ed25519_verify_rejects_tampered_signature() {
        let secret_key =
            decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let (public_key, generated_secret_key) =
            ed25519_generate_keypair(Some(&secret_key)).expect("ed25519 generate keypair");
        let mut signature =
            ed25519_sign(&generated_secret_key, b"tamper-check").expect("ed25519 sign");
        signature[0] ^= 0x01;

        assert!(
            !ed25519_verify(&public_key, b"tamper-check", &signature)
                .expect("tampered signature should still parse"),
            "tampered signature must fail verification"
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

    fn decode_hex(input: &str) -> Vec<u8> {
        let normalized: String = input
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect();
        assert_eq!(normalized.len() % 2, 0, "hex input should be even length");
        (0..normalized.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&normalized[index..index + 2], 16).unwrap())
            .collect()
    }
}
