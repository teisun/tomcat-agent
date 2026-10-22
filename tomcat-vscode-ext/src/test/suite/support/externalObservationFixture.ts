import * as assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import * as path from "node:path";

export type ExternalObservationFixture = {
  homePath: string;
  binary: string;
  workspacePath: string;
  sessionId: string;
  prefix: string;
  visible: boolean;
};

/** The external host owns this service and task; the devhost only connects. */
export async function readExternalObservationFixture(file: string): Promise<ExternalObservationFixture> {
  const value = JSON.parse(await readFile(file, "utf8")) as ExternalObservationFixture;
  for (const name of ["homePath", "binary", "workspacePath"] as const) {
    assert.ok(typeof value[name] === "string" && path.isAbsolute(value[name]), `Missing absolute ${name}`);
  }
  assert.ok(typeof value.sessionId === "string" && value.sessionId.length > 0);
  assert.ok(typeof value.prefix === "string" && value.prefix.length > 0);
  assert.equal(typeof value.visible, "boolean");
  return value;
}
