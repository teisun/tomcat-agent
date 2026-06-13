use crate::ext::WasmEngine;

#[test]
fn quickjs_runtime_exposes_tier_a_shims_and_crypto() {
    let engine = WasmEngine::global(None).expect("create quickjs engine");
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

const bytes = crypto.randomBytes(16);
if (!Buffer.isBuffer(bytes) || bytes.length !== 16) {
  throw new Error("randomBytes failed");
}

const uuid = crypto.randomUUID();
if (!/^[0-9a-f-]{36}$/.test(uuid)) {
  throw new Error("randomUUID failed: " + uuid);
}
"#;

    instance.run_script(script).expect("run shim script");
}

#[test]
fn node_fs_and_child_process_shims_fail_closed() {
    let engine = WasmEngine::global(None).expect("create quickjs engine");
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
