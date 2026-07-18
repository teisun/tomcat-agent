type HighlightJsCore = typeof import("highlight.js/lib/core");
type MermaidApi = typeof import("mermaid")["default"];

const LOG_PREFIX = "[tc-richrender]";

declare global {
  interface Window {
    __TOMCAT_RICH_RENDER_DEBUG__?: boolean;
  }
}

let highlighterPromise: Promise<HighlightJsCore["default"]> | null = null;
let mermaidPromise: Promise<MermaidApi> | null = null;
let warmupPromise: Promise<void> | null = null;

function richRenderDebugEnabled(): boolean {
  if (import.meta.env.DEV) {
    return true;
  }
  return typeof window !== "undefined" && window.__TOMCAT_RICH_RENDER_DEBUG__ === true;
}

function formatLogDetails(details: Record<string, unknown> | undefined): string {
  if (!details) {
    return "";
  }
  try {
    return JSON.stringify(details);
  } catch {
    return String(details);
  }
}

export function logRichRender(
  event: string,
  details?: Record<string, unknown>,
  level: "error" | "info" | "warn" = "info",
): void {
  if (!richRenderDebugEnabled()) {
    return;
  }
  const payload = formatLogDetails(details);
  if (payload) {
    console[level](`${LOG_PREFIX} ${event} ${payload}`);
    return;
  }
  console[level](`${LOG_PREFIX} ${event}`);
}

export async function getHighlighter(): Promise<HighlightJsCore["default"]> {
  if (!highlighterPromise) {
    logRichRender("getHighlighter: import start");
    const pending = (async () => {
      const [
        { default: hljs },
        { default: bash },
        { default: c },
        { default: cpp },
        { default: css },
        { default: diff },
        { default: go },
        { default: ini },
        { default: java },
        { default: javascript },
        { default: json },
        { default: kotlin },
        { default: markdown },
        { default: php },
        { default: plaintext },
        { default: python },
        { default: ruby },
        { default: rust },
        { default: scala },
        { default: sql },
        { default: swift },
        { default: typescript },
        { default: xml },
        { default: yaml },
      ] = await Promise.all([
        import("highlight.js/lib/core"),
        import("highlight.js/lib/languages/bash"),
        import("highlight.js/lib/languages/c"),
        import("highlight.js/lib/languages/cpp"),
        import("highlight.js/lib/languages/css"),
        import("highlight.js/lib/languages/diff"),
        import("highlight.js/lib/languages/go"),
        import("highlight.js/lib/languages/ini"),
        import("highlight.js/lib/languages/java"),
        import("highlight.js/lib/languages/javascript"),
        import("highlight.js/lib/languages/json"),
        import("highlight.js/lib/languages/kotlin"),
        import("highlight.js/lib/languages/markdown"),
        import("highlight.js/lib/languages/php"),
        import("highlight.js/lib/languages/plaintext"),
        import("highlight.js/lib/languages/python"),
        import("highlight.js/lib/languages/ruby"),
        import("highlight.js/lib/languages/rust"),
        import("highlight.js/lib/languages/scala"),
        import("highlight.js/lib/languages/sql"),
        import("highlight.js/lib/languages/swift"),
        import("highlight.js/lib/languages/typescript"),
        import("highlight.js/lib/languages/xml"),
        import("highlight.js/lib/languages/yaml"),
      ]);

      hljs.registerLanguage("bash", bash);
      hljs.registerLanguage("c", c);
      hljs.registerLanguage("cpp", cpp);
      hljs.registerLanguage("css", css);
      hljs.registerLanguage("diff", diff);
      hljs.registerLanguage("go", go);
      hljs.registerLanguage("ini", ini);
      hljs.registerLanguage("java", java);
      hljs.registerLanguage("javascript", javascript);
      hljs.registerLanguage("json", json);
      hljs.registerLanguage("kotlin", kotlin);
      hljs.registerLanguage("markdown", markdown);
      hljs.registerLanguage("php", php);
      hljs.registerLanguage("plaintext", plaintext);
      hljs.registerLanguage("python", python);
      hljs.registerLanguage("ruby", ruby);
      hljs.registerLanguage("rust", rust);
      hljs.registerLanguage("scala", scala);
      hljs.registerLanguage("sql", sql);
      hljs.registerLanguage("swift", swift);
      hljs.registerLanguage("typescript", typescript);
      hljs.registerLanguage("xml", xml);
      hljs.registerLanguage("yaml", yaml);
      hljs.registerAliases?.(["sh", "shell", "zsh"], { languageName: "bash" });
      hljs.registerAliases?.(["js", "jsx", "mjs", "cjs"], { languageName: "javascript" });
      hljs.registerAliases?.(["ts", "tsx"], { languageName: "typescript" });
      hljs.registerAliases?.(["html"], { languageName: "xml" });
      hljs.registerAliases?.(["md"], { languageName: "markdown" });
      hljs.registerAliases?.(["py"], { languageName: "python" });
      hljs.registerAliases?.(["rb"], { languageName: "ruby" });
      hljs.registerAliases?.(["rs"], { languageName: "rust" });
      hljs.registerAliases?.(["text", "txt"], { languageName: "plaintext" });
      hljs.registerAliases?.(["yml"], { languageName: "yaml" });
      logRichRender("getHighlighter: resolved", { languages: 24 });
      return hljs;
    })();
    highlighterPromise = pending.catch((error) => {
      logRichRender(
        "getHighlighter: FAILED",
        { error: error instanceof Error ? error.message : String(error) },
        "warn",
      );
      highlighterPromise = null;
      throw error;
    });
  }
  return highlighterPromise;
}

export async function getMermaid(): Promise<MermaidApi> {
  if (!mermaidPromise) {
    logRichRender("getMermaid: import start");
    mermaidPromise = import("mermaid")
      .then(({ default: mermaid }) => {
        logRichRender("getMermaid: resolved");
        return mermaid;
      })
      .catch((error) => {
        logRichRender(
          "getMermaid: FAILED",
          { error: error instanceof Error ? error.message : String(error) },
          "warn",
        );
        mermaidPromise = null;
        throw error;
      });
  }
  return mermaidPromise;
}

export function warmRichRenderModules(): Promise<void> {
  if (!warmupPromise) {
    logRichRender("warmup: start");
    warmupPromise = (async () => {
      const [highlighter, mermaid] = await Promise.allSettled([getHighlighter(), getMermaid()]);
      const details = {
        highlighter: highlighter.status === "fulfilled" ? "ok" : "FAILED",
        mermaid: mermaid.status === "fulfilled" ? "ok" : "FAILED",
      };
      if (highlighter.status === "fulfilled" && mermaid.status === "fulfilled") {
        logRichRender("warmup: ready", details);
        return;
      }
      logRichRender("warmup: FAIL", details, "warn");
      warmupPromise = null;
    })();
  }
  return warmupPromise;
}
