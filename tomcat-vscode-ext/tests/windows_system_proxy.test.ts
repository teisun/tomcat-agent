import { afterEach, beforeEach, expect, it, vi } from "vitest";

const shell = vi.hoisted(() => {
  const invoke = vi.fn();
  const execFile = Object.assign(() => {}, { [Symbol.for("nodejs.util.promisify.custom")]: invoke });
  return { invoke, execFile };
});
vi.mock("node:child_process", () => ({ execFile: shell.execFile }));
import { withWindowsSystemProxy } from "../scripts/windowsSystemProxy";

const platform = Object.getOwnPropertyDescriptor(process, "platform")!;
const saved = [
  { name: "ProxyEnable", present: false, value: null, kind: null },
  { name: "ProxyServer", present: true, value: "original.invalid:8080", kind: 1 },
  { name: "ProxyOverride", present: true, value: "localhost", kind: 1 },
];
const response = (value: unknown = "") => ({ stdout: typeof value === "string" ? value : JSON.stringify(value), stderr: "" });
beforeEach(() => {
  Object.defineProperty(process, "platform", { ...platform, value: "win32" });
  vi.stubEnv("GITHUB_ACTIONS", "true");
  shell.invoke.mockReset();
});
afterEach(() => { Object.defineProperty(process, "platform", platform); vi.unstubAllEnvs(); });

it("does not run a settings command outside the explicit Windows CI scope", async () => {
  vi.stubEnv("GITHUB_ACTIONS", undefined);
  await expect(withWindowsSystemProxy("127.0.0.1:12345", vi.fn())).rejects.toThrow("restricted");
  vi.stubEnv("GITHUB_ACTIONS", "true");
  Object.defineProperty(process, "platform", { ...platform, value: "darwin" });
  await expect(withWindowsSystemProxy("127.0.0.1:12345", vi.fn())).rejects.toThrow("restricted");
  expect(shell.invoke).not.toHaveBeenCalled();
});

it("rejects remote addresses, out-of-range ports and command injection before touching settings", async () => {
  for (const address of ["example.com:8080", "127.0.0.1:65536", "127.0.0.1:1'; throw 'injected"]) {
    await expect(withWindowsSystemProxy(address, vi.fn())).rejects.toThrow("bound loopback");
  }
  expect(shell.invoke).not.toHaveBeenCalled();
});

it("serializes the off/on control and restores original presence, values and kinds", async () => {
  shell.invoke.mockResolvedValueOnce(response(saved)).mockResolvedValueOnce(response())
    .mockResolvedValueOnce(response()).mockResolvedValueOnce(response([{ name: "ProxyEnable", value: 0 }]))
    .mockResolvedValueOnce(response()).mockResolvedValueOnce(response([{ name: "ProxyEnable", value: 1 }]))
    .mockResolvedValueOnce(response()).mockResolvedValueOnce(response(saved));
  await expect(withWindowsSystemProxy("127.0.0.1:12345", async setEnabled => {
    await setEnabled(false); await setEnabled(true); return "completed";
  })).resolves.toBe("completed");
  expect(shell.invoke).toHaveBeenCalledTimes(8);
  const script = shell.invoke.mock.calls[6][1].at(-1) as string;
  expect(script).toContain("$key.DeleteValue($item.name, $false)");
  expect(script).toContain("[Microsoft.Win32.RegistryValueKind][int]$item.kind");
  const encoded = script.match(/FromBase64String\('([^']+)'\)/)![1];
  expect(JSON.parse(Buffer.from(encoded, "base64").toString())).toEqual(saved);
});

it.each(["configure", "callback"])("restores settings when %s fails, retaining the first error", async where => {
  const primary = new Error("original acceptance error");
  shell.invoke.mockResolvedValueOnce(response(saved));
  if (where === "configure") shell.invoke.mockRejectedValueOnce(primary);
  else shell.invoke.mockResolvedValueOnce(response());
  shell.invoke.mockResolvedValueOnce(response()).mockResolvedValueOnce(response(saved));
  await expect(withWindowsSystemProxy("127.0.0.1:12345", async () => { throw primary; })).rejects.toBe(primary);
  expect(shell.invoke).toHaveBeenCalledTimes(4);
});

it("fails restoration mismatch and preserves both errors if restoration itself fails", async () => {
  shell.invoke.mockResolvedValueOnce(response(saved)).mockResolvedValueOnce(response())
    .mockResolvedValueOnce(response()).mockResolvedValueOnce(response([]));
  await expect(withWindowsSystemProxy("127.0.0.1:12345", async () => {})).rejects.toThrow("restore the original");
  const primary = new Error("acceptance error"), restore = new Error("restore error");
  shell.invoke.mockResolvedValueOnce(response(saved)).mockResolvedValueOnce(response()).mockRejectedValueOnce(restore);
  await expect(withWindowsSystemProxy("127.0.0.1:12345", async () => { throw primary; })).rejects.toMatchObject({ errors: [primary, restore] });
});
