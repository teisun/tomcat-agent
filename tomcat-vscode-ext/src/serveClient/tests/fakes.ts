import { Duplex } from "node:stream";
import { TomcatMessenger, type TomcatMessengerOptions } from "../TomcatMessenger";
import type { SharedConnectionFactory, SharedServiceInfo } from "../sharedConnection";

export const FAKE_SERVICE: SharedServiceInfo = {
  service: "tomcat", protocolVersion: 1, serverVersion: "0.1.80", instanceId: "test-service",
  pid: 4242, agentId: "main", endpoint: "test-socket", running: true,
};

export class FakeConnection extends Duplex {
  autoClose = true;
  onStderr: (chunk: Buffer) => void = () => undefined;
  private written = "";

  _read(): void {}
  _write(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error) => void): void {
    this.written += chunk.toString("utf8");
    callback();
  }
  _final(callback: () => void): void {
    callback();
    if (this.autoClose) this.push(null);
  }
  close(): void { this.push(null); this.end(); }
  emitStdout(text: string | Buffer): void { this.push(Buffer.isBuffer(text) ? text : Buffer.from(text)); }
  emitStderr(text: string): void { this.onStderr(Buffer.from(text)); }
  fail(error: Error): void { this.emit("error", error); }
  readStdin(): string {
    const value = this.written;
    this.written = "";
    return value;
  }
}

export function createConnectionFactory(socket: FakeConnection): SharedConnectionFactory {
  return async (options) => {
    socket.onStderr = options.onStderr;
    return { socket, info: FAKE_SERVICE };
  };
}

export async function createReadyMessenger(options: TomcatMessengerOptions): Promise<TomcatMessenger> {
  const messenger = new TomcatMessenger(options);
  await messenger.start();
  return messenger;
}
