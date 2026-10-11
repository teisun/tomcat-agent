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
    let secret_key = decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
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
    let secret_key = decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
    let (public_key, generated_secret_key) =
        ed25519_generate_keypair(Some(&secret_key)).expect("ed25519 generate keypair");
    let mut signature = ed25519_sign(&generated_secret_key, b"tamper-check").expect("ed25519 sign");
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
