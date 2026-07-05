import { execFileSync } from "node:child_process";
import * as path from "node:path";

import { describe, expect, it } from "vitest";

const installScriptPath = path.resolve(__dirname, "..", "..", "tomcat", "scripts", "install.sh");

function runBash(script: string): string {
  return execFileSync("bash", ["-lc", script], {
    encoding: "utf8",
  }).trim();
}

describe("install.sh release selection", () => {
  it("picks the newest CLI release instead of the newest extension release", () => {
    const releasesJson = JSON.stringify([
      {
        assets: [
          { name: "tomcat-vscode-ext-0.1.2-darwin-arm64.vsix" },
        ],
        draft: false,
        prerelease: false,
        tag_name: "ext-v0.1.2",
      },
      {
        assets: [
          { name: "tomcat-cli-v0.1.8-aarch64-apple-darwin.tar.gz" },
        ],
        draft: false,
        prerelease: false,
        tag_name: "cli-v0.1.8",
      },
    ]);

    const selectedTag = runBash(`
      export TOMCAT_INSTALL_SH_SOURCE_ONLY=1
      source ${JSON.stringify(installScriptPath)}
      curl_fetch() {
        printf '%s' ${JSON.stringify(releasesJson)}
      }
      TARGET="aarch64-apple-darwin"
      TAG=""
      load_release_metadata
      printf '%s' "$TAG"
    `);

    expect(selectedTag).toBe("cli-v0.1.8");
  });

  it("tries cli-v<ver> first and falls back to the legacy v<ver> tag", () => {
    const legacyRelease = JSON.stringify({
      assets: [
        { name: "tomcat-v0.1.7-aarch64-apple-darwin.tar.gz" },
      ],
      draft: false,
      prerelease: false,
      tag_name: "v0.1.7",
    });

    const selectedTag = runBash(`
      export TOMCAT_INSTALL_SH_SOURCE_ONLY=1
      source ${JSON.stringify(installScriptPath)}
      curl_fetch() {
        case "$1" in
          *"/releases/tags/cli-v0.1.7")
            return 22
            ;;
          *"/releases/tags/v0.1.7")
            printf '%s' ${JSON.stringify(legacyRelease)}
            ;;
          *)
            return 23
            ;;
        esac
      }
      TAG="v0.1.7"
      load_release_metadata
      printf '%s' "$TAG"
    `);

    expect(selectedTag).toBe("v0.1.7");
  });
});
