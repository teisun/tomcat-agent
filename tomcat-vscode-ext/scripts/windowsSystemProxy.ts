// Test-only Windows system settings wrapper. Never imported by product code.
import * as assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const key = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
const names = ["ProxyEnable", "ProxyServer", "ProxyOverride"];
const ps = async (body: string) => (await promisify(execFile)("powershell.exe", [
  "-NoProfile", "-NonInteractive", "-Command", `$ErrorActionPreference = 'Stop'; ${body}`,
], { timeout: 15_000, windowsHide: true })).stdout.trim();

// hyper-util 0.1.20 client/proxy/matcher.rs::win::with_system reads these exact
// CURRENT_USER values, not netsh's WinHTTP settings. No product proxy injection.
export async function withWindowsSystemProxy<T>(
  address: string,
  run: (setEnabled: (enabled: boolean) => Promise<void>) => Promise<T>,
): Promise<T> {
  if (process.platform !== "win32" || process.env.GITHUB_ACTIONS !== "true") {
    throw new Error("System-proxy mutation is restricted to the explicit GitHub Windows acceptance step");
  }
  if (!/^127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(address) || Number(address.split(":")[1]) > 65535) {
    throw new Error("Acceptance proxy must be a bound loopback HTTP proxy");
  }
  const open = `$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('${key}');`;
  const snapshot = async () => JSON.parse(await ps(`${open}
    try { @(${names.map(name => `'${name}'`).join(",")}) | ForEach-Object {
      $present = $key.GetValueNames() -contains $_;
      [pscustomobject]@{ name = $_; present = $present; value = $(if ($present) { $key.GetValue($_) } else { $null });
        kind = $(if ($present) { [int]$key.GetValueKind($_) } else { $null }) }
    } | ConvertTo-Json -Compress } finally { $key.Dispose() }`));
  const previous = await snapshot();
  const encoded = Buffer.from(JSON.stringify(previous), "utf8").toString("base64");
  let primaryFailure: unknown;
  try {
    await ps(`${open}
      try {
        $key.SetValue('ProxyServer', '${address}', [Microsoft.Win32.RegistryValueKind]::String);
        $key.SetValue('ProxyOverride', '', [Microsoft.Win32.RegistryValueKind]::String);
        $key.SetValue('ProxyEnable', 0, [Microsoft.Win32.RegistryValueKind]::DWord);
      } finally { $key.Dispose() }`);
    return await run(async enabled => {
      await ps(`${open} try { $key.SetValue('ProxyEnable', ${enabled ? 1 : 0}, [Microsoft.Win32.RegistryValueKind]::DWord) } finally { $key.Dispose() }`);
      const current = await snapshot();
      assert.equal(current.find((item: { name: string }) => item.name === "ProxyEnable").value, enabled ? 1 : 0);
    });
  } catch (error) { primaryFailure = error; throw error; }
  finally {
    try {
      await ps(`${open}
      try {
        $saved = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${encoded}')) | ConvertFrom-Json;
        foreach ($item in $saved) {
          if ($item.present) { $key.SetValue($item.name, $item.value, [Microsoft.Win32.RegistryValueKind][int]$item.kind) }
          else { $key.DeleteValue($item.name, $false) }
        }
      } finally { $key.Dispose() }`);
      assert.deepEqual(await snapshot(), previous, "restore the original registry values and value kinds");
    } catch (restoreError) {
      if (primaryFailure) throw new AggregateError([primaryFailure, restoreError], "Acceptance and system-proxy restoration both failed");
      throw restoreError;
    }
  }
}
