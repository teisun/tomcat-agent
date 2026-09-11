#!/usr/bin/env node
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const [id, ...rest] = process.argv.slice(2);
const outIndex = rest.indexOf("--out");
const out = outIndex >= 0 ? rest[outIndex + 1] : undefined;
if (!id || !out || !/^[a-z0-9][a-z0-9-]*$/.test(id)) {
  console.error("Usage: init_plugin.mjs <lowercase-hyphen-id> --out <directory>");
  process.exit(2);
}
const root = path.resolve(out, id);
await mkdir(root, { recursive: false });
await writeFile(path.join(root, "plugin.json"), `${JSON.stringify({
  id,
  name: id,
  version: "0.1.0",
  description: "TODO: explain this plugin",
  author: "TODO",
  main: "main.js",
  requiredPermissions: [],
  requiredApiVersion: "1.0",
  tags: [],
  tools: [{
    name: `${id.replaceAll("-", "_")}_example`,
    description: "TODO: replace this example tool",
    parameters: {
      type: "object",
      properties: { input: { type: "string" } },
      required: ["input"],
    },
  }],
}, null, 2)}\n`, "utf8");
await writeFile(path.join(root, "main.js"), `pi.registerTool({
  name: ${JSON.stringify(`${id.replaceAll("-", "_")}_example`)},
  description: "TODO: replace this example tool",
  parameters: {
    type: "object",
    properties: { input: { type: "string" } },
    required: ["input"],
  },
  execute: function (_toolCallId, params) {
    if (!params || typeof params.input !== "string") {
      throw new Error("input must be a string");
    }
    return { input: params.input };
  },
});
`, "utf8");
console.log(root);
