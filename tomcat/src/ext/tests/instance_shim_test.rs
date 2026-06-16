use crate::ext::PluginEngine;

#[test]
fn quickjs_runtime_exposes_tier_a_shims_and_crypto() {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("shim-crypto-test")
        .expect("create instance");

    let script = r#"
const joined = path.join("/tmp", "demo", "..", "file.txt");
if (joined !== "/tmp/file.txt") throw new Error("path.join failed: " + joined);

const relative = path.relative("/tmp/demo", "/tmp/demo/file.txt");
if (relative !== "file.txt") throw new Error("path.relative failed: " + relative);

const msg = util.format("%s:%d", "ok", 2);
if (msg !== "ok:2") throw new Error("util.format failed: " + msg);

const ee = new events.EventEmitter();
let seen = 0;
ee.on("tick", function (value) { seen = value; });
ee.emit("tick", 7);
if (seen !== 7) throw new Error("events failed");

const buf = Buffer.from("abc");
if (buf.toString("hex") !== "616263") throw new Error("buffer hex failed");
if (Buffer.concat([buf, Buffer.from("!")]).toString("utf8") !== "abc!") {
  throw new Error("buffer concat failed");
}

const digest = crypto.createHash("sha256").update("abc").digest("hex");
if (digest !== "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad") {
  throw new Error("sha256 mismatch: " + digest);
}

const mac = crypto.createHmac("sha256", "key")
  .update("The quick brown fox jumps over the lazy dog")
  .digest("hex");
if (mac !== "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8") {
  throw new Error("hmac mismatch: " + mac);
}

const bytes = crypto.randomBytes(16);
if (!Buffer.isBuffer(bytes) || bytes.length !== 16) {
  throw new Error("randomBytes failed");
}

const uuid = crypto.randomUUID();
if (!/^[0-9a-f-]{36}$/.test(uuid)) {
  throw new Error("randomUUID failed: " + uuid);
}

const aesKey = Buffer.from("00000000000000000000000000000000", "hex");
const aesIv = Buffer.from("000000000000000000000000", "hex");
const aesPlaintext = Buffer.from("00000000000000000000000000000000", "hex");
const aesSealed = crypto.aesGcmEncrypt(aesKey, aesIv, aesPlaintext);
if (aesSealed.toString("hex") !== "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf") {
  throw new Error("aes-gcm mismatch: " + aesSealed.toString("hex"));
}
const aesOpened = crypto.aesGcmDecrypt(aesKey, aesIv, aesSealed);
if (aesOpened.toString("hex") !== aesPlaintext.toString("hex")) {
  throw new Error("aes-gcm decrypt mismatch");
}

const edSeed = Buffer.from(
  "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  "hex"
);
const edPair = crypto.ed25519GenerateKeyPair(edSeed);
if (edPair.publicKey.toString("hex") !== "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a") {
  throw new Error("ed25519 public key mismatch");
}
const edSignature = crypto.ed25519Sign(edPair.secretKey, Buffer.alloc(0));
if (edSignature.toString("hex") !== "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b") {
  throw new Error("ed25519 signature mismatch");
}
if (!crypto.ed25519Verify(edPair.publicKey, Buffer.alloc(0), edSignature)) {
  throw new Error("ed25519 verify failed");
}
"#;

    instance.run_script(script).expect("run shim script");
}

#[test]
fn node_fs_and_child_process_shims_fail_closed() {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("sensitive-shim-test")
        .expect("create instance");

    let script = r#"
let fsFailed = false;
try {
  __node_fs.readFileSync("/tmp/demo.txt", "utf8");
} catch (error) {
  fsFailed = String(error).indexOf("pi.readFile") >= 0;
}
if (!fsFailed) throw new Error("node:fs should fail closed");

let execFailed = false;
try {
  __node_child_process.execSync("echo hi");
} catch (error) {
  execFailed = String(error).indexOf("pi.exec") >= 0;
}
if (!execFailed) throw new Error("node:child_process should fail closed");
"#;

    instance.run_script(script).expect("run fail-closed script");
}

#[test]
fn crypto_namespace_objects_match_flat_api() {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("crypto-namespace-test")
        .expect("create instance");

    let script = r#"
if (crypto.aesGcm.encrypt !== crypto.aesGcmEncrypt) {
  throw new Error("crypto.aesGcm.encrypt alias mismatch");
}
if (crypto.aesGcm.decrypt !== crypto.aesGcmDecrypt) {
  throw new Error("crypto.aesGcm.decrypt alias mismatch");
}
if (crypto.ed25519.generateKeyPair !== crypto.ed25519GenerateKeyPair) {
  throw new Error("crypto.ed25519.generateKeyPair alias mismatch");
}
if (crypto.ed25519.sign !== crypto.ed25519Sign) {
  throw new Error("crypto.ed25519.sign alias mismatch");
}
if (crypto.ed25519.verify !== crypto.ed25519Verify) {
  throw new Error("crypto.ed25519.verify alias mismatch");
}

const aesKey = Buffer.from("00000000000000000000000000000000", "hex");
const aesIv = Buffer.from("000000000000000000000000", "hex");
const aesPlaintext = Buffer.from("00000000000000000000000000000000", "hex");
const aesSealed = crypto.aesGcm.encrypt(aesKey, aesIv, aesPlaintext);
if (aesSealed.toString("hex") !== "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf") {
  throw new Error("crypto.aesGcm.encrypt mismatch: " + aesSealed.toString("hex"));
}
const aesOpened = crypto.aesGcm.decrypt(aesKey, aesIv, aesSealed);
if (aesOpened.toString("hex") !== aesPlaintext.toString("hex")) {
  throw new Error("crypto.aesGcm.decrypt mismatch");
}

const edSeed = Buffer.from(
  "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  "hex"
);
const edPair = crypto.ed25519.generateKeyPair(edSeed);
const edSignature = crypto.ed25519.sign(edPair.secretKey, Buffer.alloc(0));
if (!crypto.ed25519.verify(edPair.publicKey, Buffer.alloc(0), edSignature)) {
  throw new Error("crypto.ed25519 namespace verify failed");
}
"#;

    instance
        .run_script(script)
        .expect("run namespace shim script");
}

#[test]
fn crypto_native_rejects_invalid_base64() {
    let engine = PluginEngine::global(None).expect("create quickjs engine");
    let mut instance = engine
        .create_instance("crypto-bad-input-test")
        .expect("create instance");

    let script = r#"
let badAesError = "";
try {
  globalThis.__pi_crypto_aes_gcm_encrypt_native("%%%not-base64%%%", "", "", "");
} catch (error) {
  badAesError = String(error);
}
if (badAesError.indexOf("invalid base64 input") < 0) {
  throw new Error("invalid aes input should surface base64 error: " + badAesError);
}

let badEdError = "";
try {
  globalThis.__pi_crypto_ed25519_sign_native("%%%not-base64%%%", "%%%not-base64%%%");
} catch (error) {
  badEdError = String(error);
}
if (badEdError.indexOf("invalid base64 input") < 0) {
  throw new Error("invalid ed25519 input should surface base64 error: " + badEdError);
}
"#;

    instance
        .run_script(script)
        .expect("run invalid base64 error script");
}
