import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { bundledExecutableRelativePath } from "../scripts/package-vsix";
import { PE_MACHINE_AMD64, peMachine, vscodeTargetFor } from "../scripts/run-vscode-verify-vsix";

function peImage(machine: number): Buffer {
  const image = Buffer.alloc(0x100);
  image.write("MZ", 0, "latin1");
  image.writeUInt32LE(0x80, 0x3c);
  image.write("PE\0\0", 0x80, "latin1");
  image.writeUInt16LE(machine, 0x84);
  return image;
}

describe("release verify target", () => {
  it("maps a Windows x64 host to the win32-x64 bundle with tomcat.exe", () => {
    expect(vscodeTargetFor("win32", "x64")).toBe("win32-x64");
    expect(bundledExecutableRelativePath(vscodeTargetFor("win32", "x64"))).toBe("bin/tomcat.exe");
    expect(vscodeTargetFor("linux", "x64")).toBe("linux-x64");
    expect(() => vscodeTargetFor("win32", "arm64")).toThrow(/Unsupported local verify platform: win32\/arm64/);
  });

  it("accepts only an AMD64 PE image", () => {
    expect(peMachine(peImage(PE_MACHINE_AMD64))).toBe(PE_MACHINE_AMD64);
    expect(peMachine(peImage(0xaa64))).not.toBe(PE_MACHINE_AMD64);
    const elf = Buffer.alloc(0x100);
    elf.write("\x7fELF", 0, "latin1");
    expect(peMachine(elf)).toBeUndefined();
    const truncated = peImage(PE_MACHINE_AMD64);
    truncated.writeUInt32LE(0xfff0, 0x3c);
    expect(peMachine(truncated)).toBeUndefined();
  });

  it.runIf(process.platform === "win32" && process.arch === "x64")("reads the machine of a real x64 executable", () => {
    expect(peMachine(readFileSync(process.execPath))).toBe(PE_MACHINE_AMD64);
  });
});
