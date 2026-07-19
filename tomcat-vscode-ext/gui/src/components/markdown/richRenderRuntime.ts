import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import plaintext from "highlight.js/lib/languages/plaintext";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import scala from "highlight.js/lib/languages/scala";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

type MermaidApi = typeof import("mermaid")["default"];

const LOG_PREFIX = "[tc-richrender]";

declare global {
  interface Window {
    __TOMCAT_RICH_RENDER_DEBUG__?: boolean;
  }
}

let mermaidPromise: Promise<MermaidApi> | null = null;
let warmupPromise: Promise<void> | null = null;
let highlightLanguagesRegistered = false;

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

function ensureHighlightLanguagesRegistered(): void {
  if (highlightLanguagesRegistered) {
    return;
  }
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
  highlightLanguagesRegistered = true;
  logRichRender("highlight: register ready", { languages: 24 });
}

export function highlightToHtml(
  code: string,
  requestedLanguage: string,
): { html: string; language: string } {
  ensureHighlightLanguagesRegistered();
  const language = hljs.getLanguage(requestedLanguage) ? requestedLanguage : "plaintext";
  try {
    const html = hljs.highlight(code, { ignoreIllegals: true, language }).value;
    logRichRender("highlight: sync", {
      language,
      requestedLanguage,
      textLength: code.length,
    });
    return { html, language };
  } catch (error) {
    logRichRender(
      "highlight: sync FAILED",
      {
        error: error instanceof Error ? error.message : String(error),
        requestedLanguage,
      },
      "warn",
    );
    return {
      html: hljs.highlight(code, { ignoreIllegals: true, language: "plaintext" }).value,
      language: "plaintext",
    };
  }
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
    warmupPromise = getMermaid()
      .then(() => {
        logRichRender("warmup: ready", { mermaid: "ok" });
      })
      .catch((error) => {
        logRichRender(
          "warmup: FAIL",
          { error: error instanceof Error ? error.message : String(error), mermaid: "FAILED" },
          "warn",
        );
        warmupPromise = null;
        throw error;
      });
  }
  return warmupPromise;
}

ensureHighlightLanguagesRegistered();
