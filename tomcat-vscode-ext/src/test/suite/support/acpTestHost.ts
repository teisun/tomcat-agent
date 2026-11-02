import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

/** Minimal standard ACP test peer; the real tomcat acp/serve owns execution. */
export function startAcpTestHost(binary: string, cwd: string, env: NodeJS.ProcessEnv) {
  const child = spawn(binary, ["acp"], { cwd, env, stdio: "pipe" });
  const updates: Record<string, unknown>[] = [];
  const pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void; timer: NodeJS.Timeout }>();
  let nextId = 1;
  let stderr = "";
  const fail = (error: Error) => {
    for (const request of pending.values()) { clearTimeout(request.timer); request.reject(error); }
    pending.clear();
  };
  child.stderr.on("data", chunk => { stderr = (stderr + String(chunk)).slice(-64_000); });
  child.stdin.on("error", fail);
  const exited = new Promise<void>((resolve, reject) => {
    child.once("error", error => { fail(error); reject(error); });
    child.once("close", (code, signal) => {
      const error = new Error(`ACP fixture exited (${code ?? signal}): ${stderr}`);
      fail(error);
      if (code === 0) resolve(); else reject(error);
    });
  });
  void exited.catch(() => undefined);
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  lines.on("line", line => {
    try {
      const frame = JSON.parse(line) as Record<string, unknown>;
      if (frame.method === "session/request_permission") {
        // Unexpected prompts never grant a test process additional permissions.
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { outcome: { outcome: "cancelled" } } })}\n`);
      } else if (frame.method === "session/update") {
        updates.push(frame.params as Record<string, unknown>);
      } else if (typeof frame.id === "number") {
        const request = pending.get(frame.id);
        if (!request) return;
        pending.delete(frame.id);
        clearTimeout(request.timer);
        if (frame.error) request.reject(new Error(JSON.stringify(frame.error)));
        else request.resolve(frame.result);
      }
    } catch (error) { fail(error instanceof Error ? error : new Error(String(error))); }
  });
  return {
    updates,
    get stderr() { return stderr; },
    request<T = Record<string, unknown>>(method: string, params: Record<string, unknown>, timeoutMs = 30_000): Promise<T> {
      return new Promise((resolve, reject) => {
        if (!child.stdin.writable) { reject(new Error(`ACP fixture is closed: ${stderr}`)); return; }
        const id = nextId++;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`ACP ${method} timed out: ${stderr}`)); }, timeoutMs);
        pending.set(id, { resolve: value => resolve(value as T), reject, timer });
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      });
    },
    async close() {
      child.stdin.end();
      let timer: NodeJS.Timeout | undefined;
      try {
        await Promise.race([exited, new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => { child.kill("SIGKILL"); reject(new Error("ACP fixture did not drain after stdin EOF")); }, 20_000);
        })]);
      } finally { clearTimeout(timer); lines.close(); }
    },
  };
}
