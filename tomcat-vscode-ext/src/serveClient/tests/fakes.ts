import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";

import type { ChildProcessWithoutNullStreams } from "node:child_process";

export class FakeChildProcess extends EventEmitter {
  public readonly stdin = new PassThrough();
  public readonly stdout = new PassThrough();
  public readonly stderr = new PassThrough();
  public exitCode: number | null = null;
  public killed = false;
  public pid = 4242;
  public signalCode: NodeJS.Signals | null = null;

  kill(signal?: NodeJS.Signals): boolean {
    this.killed = true;
    this.signalCode = signal ?? null;
    this.exitCode = 0;
    this.emit("exit", this.exitCode, this.signalCode);
    return true;
  }

  emitStdout(text: string): void {
    this.stdout.write(text, "utf8");
  }

  emitStderr(text: string): void {
    this.stderr.write(text, "utf8");
  }

  fail(error: Error): void {
    this.emit("error", error);
  }

  readStdin(): string {
    return this.stdin.read()?.toString("utf8") ?? "";
  }
}

export function createSpawnFactory(child: FakeChildProcess) {
  return (() =>
    child as unknown as ChildProcessWithoutNullStreams) as unknown as typeof import("node:child_process").spawn;
}
