(function () {
  'use strict';

  function ensureNative(name) {
    if (typeof globalThis[name] !== 'function') {
      throw new Error(name + ' is not available in this QuickJS runtime');
    }
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

  var crypto = globalThis.crypto || {};
  crypto.createHash = createHash;
  crypto.randomBytes = randomBytes;
  crypto.randomUUID = randomUUID;
  crypto["default"] = crypto;

  globalThis.crypto = crypto;
})();
