(function () {
  'use strict';

  function ensureNative(name) {
    if (typeof globalThis[name] !== 'function') {
      throw new Error(name + ' is not available in this QuickJS runtime');
    }
  }

  function toBuffer(input, encoding) {
    if (globalThis.Buffer.isBuffer(input)) {
      return input;
    }
    if (input instanceof Uint8Array) {
      return globalThis.Buffer.from(input);
    }
    return globalThis.Buffer.from(input, encoding);
  }

  function optionalToBase64(input, encoding) {
    if (typeof input === 'undefined' || input === null) {
      return '';
    }
    return toBuffer(input, encoding).toString('base64');
  }

  function createHash(algo) {
    var chunks = [];
    var normalizedAlgo = String(algo || 'sha256').toLowerCase();
    return {
      update: function (input, encoding) {
        chunks.push(globalThis.Buffer.from(input, encoding));
        return this;
      },
      digest: function (encoding) {
        ensureNative('__pi_crypto_hash_native');
        var merged = globalThis.Buffer.concat(chunks);
        var hex = globalThis.__pi_crypto_hash_native(
          normalizedAlgo,
          merged.toString('base64')
        );
        if (typeof encoding === 'undefined') {
          return globalThis.Buffer.from(hex, 'hex');
        }
        return globalThis.Buffer.from(hex, 'hex').toString(encoding);
      }
    };
  }

  function createHmac(algo, key) {
    var chunks = [];
    var normalizedAlgo = String(algo || 'sha256').toLowerCase();
    var keyBytes = globalThis.Buffer.from(key);
    return {
      update: function (input, encoding) {
        chunks.push(globalThis.Buffer.from(input, encoding));
        return this;
      },
      digest: function (encoding) {
        ensureNative('__pi_crypto_hmac_native');
        var merged = globalThis.Buffer.concat(chunks);
        var hex = globalThis.__pi_crypto_hmac_native(
          normalizedAlgo,
          keyBytes.toString('base64'),
          merged.toString('base64')
        );
        if (typeof encoding === 'undefined') {
          return globalThis.Buffer.from(hex, 'hex');
        }
        return globalThis.Buffer.from(hex, 'hex').toString(encoding);
      }
    };
  }

  function randomBytes(size) {
    ensureNative('__pi_crypto_random_bytes_native');
    return globalThis.Buffer.from(
      globalThis.__pi_crypto_random_bytes_native(size >>> 0),
      'hex'
    );
  }

  function randomUUID() {
    ensureNative('__pi_crypto_random_uuid_native');
    return globalThis.__pi_crypto_random_uuid_native();
  }

  function aesGcmEncrypt(key, iv, plaintext, aad) {
    ensureNative('__pi_crypto_aes_gcm_encrypt_native');
    return globalThis.Buffer.from(
      globalThis.__pi_crypto_aes_gcm_encrypt_native(
        toBuffer(key).toString('base64'),
        toBuffer(iv).toString('base64'),
        toBuffer(plaintext).toString('base64'),
        optionalToBase64(aad)
      ),
      'base64'
    );
  }

  function aesGcmDecrypt(key, iv, ciphertextAndTag, aad) {
    ensureNative('__pi_crypto_aes_gcm_decrypt_native');
    return globalThis.Buffer.from(
      globalThis.__pi_crypto_aes_gcm_decrypt_native(
        toBuffer(key).toString('base64'),
        toBuffer(iv).toString('base64'),
        toBuffer(ciphertextAndTag).toString('base64'),
        optionalToBase64(aad)
      ),
      'base64'
    );
  }

  function ed25519GenerateKeyPair(seed) {
    ensureNative('__pi_crypto_ed25519_generate_keypair_native');
    var response = JSON.parse(
      globalThis.__pi_crypto_ed25519_generate_keypair_native(
        optionalToBase64(seed)
      )
    );
    return {
      publicKey: globalThis.Buffer.from(response.publicKey, 'base64'),
      secretKey: globalThis.Buffer.from(response.secretKey, 'base64')
    };
  }

  function ed25519Sign(secretKey, data) {
    ensureNative('__pi_crypto_ed25519_sign_native');
    return globalThis.Buffer.from(
      globalThis.__pi_crypto_ed25519_sign_native(
        toBuffer(secretKey).toString('base64'),
        toBuffer(data).toString('base64')
      ),
      'base64'
    );
  }

  function ed25519Verify(publicKey, data, signature) {
    ensureNative('__pi_crypto_ed25519_verify_native');
    return !!globalThis.__pi_crypto_ed25519_verify_native(
      toBuffer(publicKey).toString('base64'),
      toBuffer(data).toString('base64'),
      toBuffer(signature).toString('base64')
    );
  }

  var crypto = globalThis.crypto || {};
  crypto.createHash = createHash;
  crypto.createHmac = createHmac;
  crypto.randomBytes = randomBytes;
  crypto.randomUUID = randomUUID;
  crypto.aesGcmEncrypt = aesGcmEncrypt;
  crypto.aesGcmDecrypt = aesGcmDecrypt;
  crypto.ed25519GenerateKeyPair = ed25519GenerateKeyPair;
  crypto.ed25519Sign = ed25519Sign;
  crypto.ed25519Verify = ed25519Verify;
  crypto.aesGcm = {
    encrypt: aesGcmEncrypt,
    decrypt: aesGcmDecrypt
  };
  crypto.ed25519 = {
    generateKeyPair: ed25519GenerateKeyPair,
    sign: ed25519Sign,
    verify: ed25519Verify
  };
  crypto["default"] = crypto;

  globalThis.crypto = crypto;
})();
