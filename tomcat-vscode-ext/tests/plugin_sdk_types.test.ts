import * as path from "node:path";
import ts from "typescript";
import { expect, it } from "vitest";

it("typechecks plugin SDK consumers without relaxing JSON or the full-shell command contract", () => {
  const sdk = path.resolve(__dirname, "../../tomcat/assets/types/tomcat-plugin.d.ts");
  const fixture = path.resolve(__dirname, "plugin-sdk-consumer.ts");
  const source = `
    tomcat.exec("git status --short");
    tomcat.exec("git status --short", [], { cwd: "/workspace", foregroundWaitMs: 9000 });
    pi.exec("git status --short", undefined, { cwd: "/workspace" });
    tomcat.createChatCompletion({
      messages: [{ role: "user", content: [{
        type: "text", text: "hello", annotations: [{ type: undefined, url: undefined }]
      }] }],
      tools: [{ name: "echo", parameters: { type: "object", optional: undefined } }]
    });
    tomcat.fetch({ url: "https://example.com", query: { optional: undefined, enabled: true } });
    // @ts-expect-error Separate argv was deliberately retired.
    tomcat.exec("git", ["status"]);
    // @ts-expect-error Options stay in the existing third position.
    tomcat.exec("git status", { cwd: "/workspace" });
    // @ts-expect-error Optional object properties do not permit functions.
    tomcat.emit("bad", { callback: () => 1 });
    // @ts-expect-error Array entries must still be JSON values.
    tomcat.emit("bad", [undefined]);
  `;
  const options: ts.CompilerOptions = {
    noEmit: true,
    strict: true,
    target: ts.ScriptTarget.ES2022,
    types: [],
  };
  const host = ts.createCompilerHost(options);
  const getSourceFile = host.getSourceFile;
  host.getSourceFile = (file, ...args) => file === fixture
    ? ts.createSourceFile(file, source, ts.ScriptTarget.ES2022, true)
    : getSourceFile.call(host, file, ...args);
  const program = ts.createProgram([sdk, fixture], options, host);
  const diagnostics = ts.getPreEmitDiagnostics(program);
  expect(ts.formatDiagnosticsWithColorAndContext(diagnostics, {
    getCanonicalFileName: (file) => file,
    getCurrentDirectory: () => process.cwd(),
    getNewLine: () => "\n",
  })).toBe("");
});
