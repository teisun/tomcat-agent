// Test-only, explicitly opted-in Windows runner PATH acceptance. No product import.
import * as assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const ps = async (body: string) => (await promisify(execFile)("powershell.exe", [
  "-NoProfile", "-NonInteractive", "-Command", `$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); ${body}`,
], { timeout: 15_000, windowsHide: true })).stdout.trim();
const open = "$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true);";
type UserPath = { present: boolean; value: string | null; kind: number | null };
export async function readWindowsUserPath(): Promise<UserPath> {
  return JSON.parse(await ps(`${open} try {
    $present = $null -ne $key -and $key.GetValueNames() -contains 'Path';
    [pscustomobject]@{ present = $present; value = $(if ($present) { $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } else { $null });
      kind = $(if ($present) { [int]$key.GetValueKind('Path') } else { $null }) } | ConvertTo-Json -Compress
  } finally { if ($null -ne $key) { $key.Dispose() } }`));
}
export async function windowsLoginPath(): Promise<string> {
  return ps("[Environment]::ExpandEnvironmentVariables([Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User'))");
}
export async function withWindowsUserPath<T>(run: () => Promise<T>, report: (value: unknown) => Promise<void>): Promise<T> {
  assert.equal(process.platform, "win32");
  assert.equal(process.env.GITHUB_ACTIONS, "true");
  assert.equal(process.env.RUNNER_ENVIRONMENT, "github-hosted", "only the approved disposable hosted runner account");
  assert.ok(process.argv.includes("--allow-runner-path"), "explicit runner PATH opt-in is required");
  const previous = await readWindowsUserPath();
  const encoded = Buffer.from(JSON.stringify(previous), "utf8").toString("base64");
  let failure: unknown;
  try { return await run(); }
  catch (error) { failure = error; throw error; }
  finally {
    try {
      await ps(`${open} try {
        $saved = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${encoded}')) | ConvertFrom-Json;
        if ($saved.present) {
          if ($null -eq $key) { $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment') }
          $key.SetValue('Path', $saved.value, [Microsoft.Win32.RegistryValueKind][int]$saved.kind)
        } elseif ($null -ne $key) { $key.DeleteValue('Path', $false) }
      } finally { if ($null -ne $key) { $key.Dispose() } }`);
      const restored = await readWindowsUserPath();
      assert.deepEqual(restored, previous, "independently reread PATH value, type and original presence after restoration");
      await report({ status: "restored", previous, restored });
    } catch (restoreError) {
      if (failure) throw new AggregateError([failure, restoreError], "Acceptance and user PATH restoration failed");
      throw restoreError;
    }
  }
}
