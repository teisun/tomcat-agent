import { createRequire } from "node:module";
import * as path from "node:path";

/**
 * Same extractor Cursor / VS Code uses to install a VSIX.
 * Evidence: vscode/src/vs/base/node/zip.ts `extract`, `openZip`, `openZipStream`.
 *
 * Walking the central directory is not enough: the install failure we hit was
 * local headers that did not match the directory. `openReadStream` is the
 * installer's actual read path.
 */
type YauzlZip = {
  close(): void;
  on(event: "end", listener: () => void): void;
  on(event: "error", listener: (error: Error) => void): void;
  on(event: "entry", listener: (entry: { fileName: string }) => void): void;
  openReadStream(
    entry: { fileName: string },
    callback: (error: Error | null, stream?: NodeJS.ReadableStream) => void,
  ): void;
  readEntry(): void;
};

const yauzl = createRequire(__filename)("yauzl") as {
  open(
    filePath: string,
    options: { autoClose: boolean; lazyEntries: boolean },
    callback: (error: Error | null, zip?: YauzlZip) => void,
  ): void;
};

export function extractVsixLikeCursor(vsixPath: string): Promise<void> {
  return new Promise((resolve, reject) => {
    yauzl.open(vsixPath, { autoClose: true, lazyEntries: true }, (error, zip) => {
      if (error || !zip) {
        reject(error ?? new Error(`failed to open ${vsixPath}`));
        return;
      }

      let settled = false;
      const fail = (cause: Error): void => {
        if (settled) {
          return;
        }
        settled = true;
        zip.close();
        reject(cause);
      };

      zip.on("error", fail);
      zip.on("end", () => {
        if (!settled) {
          settled = true;
          resolve();
        }
      });
      zip.on("entry", (entry) => {
        if (entry.fileName.endsWith("/")) {
          zip.readEntry();
          return;
        }
        zip.openReadStream(entry, (streamError, stream) => {
          if (streamError || !stream) {
            fail(streamError ?? new Error(`cannot read ${entry.fileName}`));
            return;
          }
          stream.on("error", (streamCause: Error) => fail(streamCause));
          stream.on("end", () => zip.readEntry());
          stream.resume();
        });
      });
      zip.readEntry();
    });
  });
}

function main(): void {
  const vsixPath = process.argv[2];
  if (!vsixPath) {
    console.error("usage: vsix-extractable <file.vsix>");
    process.exit(2);
    return;
  }
  extractVsixLikeCursor(vsixPath).catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(2);
  });
}

if (path.basename(process.argv[1] ?? "") === "vsix-extractable.ts") {
  main();
}
