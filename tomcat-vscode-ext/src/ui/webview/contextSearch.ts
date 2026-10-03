import * as os from "node:os";
import * as path from "node:path";

import * as vscode from "vscode";

import { TOMCAT_CONFIG_SECTION } from "../../constants";
import { buildFileReference } from "./contextReferences";
import type { ContextSearchMatch } from "./protocol";
import type { PathResolution } from "../../shared/pathResolution";

const DEFAULT_CONTEXT_SEARCH_LIMIT = 20;
const DEFAULT_CONTEXT_SEARCH_MAX_FILES = 20_000;

export const CONTEXT_SEARCH_DISABLE_ENV = "TOMCAT_CONTEXT_SEARCH_DISABLE";
export const CONTEXT_SEARCH_LIMIT_ENV = "TOMCAT_CONTEXT_SEARCH_LIMIT";
export const CONTEXT_SEARCH_MAX_FILES_ENV = "TOMCAT_CONTEXT_SEARCH_MAX_FILES";

interface ContextSearchCache {
  candidates: CachedSearchCandidate[];
  maxFiles: number;
  truncated: boolean;
}

export interface ContextSearchConfig {
  disabled: boolean;
  limit: number;
  maxFiles: number;
}

export interface ContextSearchResult {
  matches: ContextSearchMatch[];
  truncated: boolean;
  workspaceAvailable: boolean;
}

export interface ContextSearchRequest {
  kind?: "file";
  query: string;
  token?: vscode.CancellationToken;
}

export interface PathResolveRequest {
  /** Directory used to resolve relative paths; omit for the first workspace folder. */
  relativeBase?: string;
  paths: readonly string[];
}

export interface SearchCandidate {
  displayPath: string;
  isDirectory: boolean;
  isOpen: boolean;
  label: string;
  uri: vscode.Uri;
}

interface CachedSearchCandidate extends Omit<SearchCandidate, "isOpen"> {
  match: ContextSearchMatch;
  normalizedFsPath: string;
}

function normalizePath(value: string): string {
  return value.replace(/\\/g, "/");
}

function resolveCandidatePath(value: string, relativeBase?: string): string {
  const trimmed = value.trim();
  if (path.isAbsolute(trimmed)) {
    return path.normalize(trimmed);
  }
  if (trimmed.startsWith("~/")) {
    return path.join(os.homedir(), trimmed.slice(2));
  }
  if (relativeBase) {
    return path.resolve(relativeBase, trimmed);
  }
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  return path.resolve(workspaceRoot ?? process.cwd(), trimmed);
}

function parsePositiveInteger(value: string | undefined): number | undefined {
  if (!value) {
    return undefined;
  }
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
}

function parseBooleanFlag(value: string | undefined): boolean | undefined {
  if (!value) {
    return undefined;
  }
  switch (value.trim().toLowerCase()) {
    case "1":
    case "true":
    case "yes":
    case "on":
      return true;
    case "0":
    case "false":
    case "no":
    case "off":
      return false;
    default:
      return undefined;
  }
}

function sanitizeConfiguredMaxFiles(value: unknown): number | undefined {
  return typeof value === "number" && Number.isInteger(value) && value > 0
    ? value
    : undefined;
}

export function readContextSearchConfig(): ContextSearchConfig {
  const config = vscode.workspace.getConfiguration(TOMCAT_CONFIG_SECTION);
  return {
    disabled: parseBooleanFlag(process.env[CONTEXT_SEARCH_DISABLE_ENV]) ?? false,
    limit:
      parsePositiveInteger(process.env[CONTEXT_SEARCH_LIMIT_ENV])
      ?? DEFAULT_CONTEXT_SEARCH_LIMIT,
    maxFiles:
      parsePositiveInteger(process.env[CONTEXT_SEARCH_MAX_FILES_ENV])
      ?? sanitizeConfiguredMaxFiles(config.get("contextSearch.maxFiles"))
      ?? DEFAULT_CONTEXT_SEARCH_MAX_FILES,
  };
}

function resolveWorkspaceRoot(uri: vscode.Uri): string | null {
  const normalizedUriPath = normalizePath(uri.fsPath);
  let match: string | null = null;
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const candidate = normalizePath(folder.uri.fsPath).replace(/\/+$/u, "");
    if (
      normalizedUriPath === candidate ||
      normalizedUriPath.startsWith(`${candidate}/`)
    ) {
      if (!match || candidate.length > match.length) {
        match = candidate;
      }
    }
  }
  return match;
}

function withUriPath(baseUri: vscode.Uri, nextPath: string): vscode.Uri {
  return vscode.Uri.from({
    authority: baseUri.authority,
    path: normalizePath(nextPath),
    query: baseUri.query,
    scheme: baseUri.scheme,
  });
}

export function deriveDirectories(files: readonly vscode.Uri[]): vscode.Uri[] {
  const directories = new Map<string, vscode.Uri>();
  // Known limitation by design: directories are derived from indexed files,
  // so empty folders are not offered as @ candidates.
  for (const file of files) {
    const workspaceRoot = resolveWorkspaceRoot(file);
    if (!workspaceRoot) {
      continue;
    }
    const normalizedRoot = normalizePath(workspaceRoot);
    let current = path.dirname(file.fsPath);
    while (true) {
      const normalizedCurrent = normalizePath(current).replace(/\/+$/u, "");
      if (!normalizedCurrent || normalizedCurrent === normalizedRoot) {
        break;
      }
      if (!directories.has(normalizedCurrent)) {
        directories.set(normalizedCurrent, withUriPath(file, current));
      }
      const parent = path.dirname(current);
      if (parent === current) {
        break;
      }
      current = parent;
    }
  }
  return [...directories.values()].sort((left, right) => left.fsPath.localeCompare(right.fsPath));
}

function describeRelativeParent(displayPath: string): string | null {
  const normalized = normalizePath(displayPath).replace(/\/+$/u, "");
  if (!normalized) {
    return null;
  }
  const parent = path.posix.dirname(normalized);
  return parent === "." ? null : parent;
}

function subsequenceScore(candidate: SearchCandidate, query: string): number | null {
  if (!query) {
    return 0;
  }

  const displayPath = candidate.displayPath;
  const lowerDisplayPath = displayPath.toLowerCase();
  const lowerLabel = candidate.label.toLowerCase();
  const lowerQuery = query.toLowerCase();
  const trimmedLabel = candidate.label.replace(/\/+$/u, "");
  const basenameStart = lowerDisplayPath.length - trimmedLabel.length;

  let score = 0;
  let lastIndex = -1;
  for (let index = 0; index < lowerQuery.length; index += 1) {
    const char = lowerQuery[index];
    const foundIndex = lowerDisplayPath.indexOf(char, lastIndex + 1);
    if (foundIndex < 0) {
      return null;
    }
    score += 10;
    if (foundIndex === lastIndex + 1) {
      score += 8;
    }
    if (foundIndex >= basenameStart) {
      score += 12;
    }
    const previousChar = foundIndex > 0 ? lowerDisplayPath[foundIndex - 1] : "/";
    if ("/._- ".includes(previousChar)) {
      score += 6;
    }
    if (displayPath[foundIndex] === query[index]) {
      score += 1;
    }
    lastIndex = foundIndex;
  }

  if (lowerLabel.startsWith(lowerQuery)) {
    score += 24;
  } else if (lowerDisplayPath.startsWith(lowerQuery)) {
    score += 18;
  }

  if (candidate.isDirectory && query.endsWith("/")) {
    score += 10;
  }

  return score;
}

export function fuzzyRank<T extends SearchCandidate>(
  candidates: readonly T[],
  query: string,
): T[] {
  const ranked = candidates
    .map((candidate) => ({
      candidate,
      score: subsequenceScore(candidate, query),
    }))
    .filter((entry): entry is { candidate: T; score: number } => entry.score !== null)
    .sort((left, right) => {
      if (right.score !== left.score) {
        return right.score - left.score;
      }
      if (left.candidate.isOpen !== right.candidate.isOpen) {
        return left.candidate.isOpen ? -1 : 1;
      }
      if (left.candidate.displayPath.length !== right.candidate.displayPath.length) {
        return left.candidate.displayPath.length - right.candidate.displayPath.length;
      }
      return left.candidate.displayPath.localeCompare(right.candidate.displayPath);
    });

  return ranked.map((entry) => entry.candidate);
}

function collectOpenPaths(): Set<string> {
  const paths = new Set<string>();
  for (const editor of vscode.window.visibleTextEditors) {
    paths.add(normalizePath(editor.document.uri.fsPath));
  }
  for (const document of vscode.workspace.textDocuments) {
    paths.add(normalizePath(document.uri.fsPath));
  }
  return paths;
}

function buildCachedCandidate(uri: vscode.Uri, isDirectory: boolean): CachedSearchCandidate {
  const reference = buildFileReference(uri, { isDirectory });
  const displayPath = normalizePath(reference.path);
  return {
    displayPath,
    isDirectory,
    label: reference.label,
    match: {
      description: describeRelativeParent(displayPath),
      reference,
    },
    normalizedFsPath: normalizePath(uri.fsPath),
    uri,
  };
}

export class ContextSearchService implements vscode.Disposable {
  private cache: ContextSearchCache | null = null;
  private readonly directPathCache = new Map<string, PathResolution["kind"]>();
  private dirty = false;
  private indexRevision = 0;
  private readonly watcher: vscode.FileSystemWatcher | null;

  constructor() {
    this.watcher = vscode.workspace.workspaceFolders?.length
      ? vscode.workspace.createFileSystemWatcher("**/*")
      : null;
    this.watcher?.onDidCreate(() => {
      this.markDirty();
    });
    this.watcher?.onDidDelete(() => {
      this.markDirty();
    });
  }

  dispose(): void {
    this.watcher?.dispose();
  }

  async search(request: ContextSearchRequest): Promise<ContextSearchResult> {
    const config = readContextSearchConfig();
    const workspaceAvailable = (vscode.workspace.workspaceFolders?.length ?? 0) > 0;
    if (!workspaceAvailable || config.disabled || (request.kind && request.kind !== "file")) {
      return {
        matches: [],
        truncated: false,
        workspaceAvailable,
      };
    }

    const cache = await this.ensureCache(config.maxFiles, request.token);
    if (request.token?.isCancellationRequested) {
      return {
        matches: [],
        truncated: false,
        workspaceAvailable,
      };
    }

    const openPaths = collectOpenPaths();
    const ranked = fuzzyRank(
      cache.candidates.map((candidate) => ({
        ...candidate,
        isOpen: !candidate.isDirectory && openPaths.has(candidate.normalizedFsPath),
      })),
      request.query.trim(),
    );
    const truncated =
      ranked.length > config.limit || (cache.truncated && ranked.length >= config.limit);

    return {
      matches: ranked.slice(0, config.limit).map((candidate) => candidate.match),
      truncated,
      workspaceAvailable,
    };
  }

  /**
   * Resolve free-form Markdown path candidates for a rendered webview.
   *
   * The @-mention index is consulted when it is already warm. We intentionally
   * do not build a 20,000-file index merely to validate a handful of path
   * chips; cold requests fall back to `workspace.fs.stat` and are cached until
   * the existing filesystem watcher reports a create/delete change.
   */
  async resolvePaths(request: PathResolveRequest): Promise<PathResolution[]> {
    const indexedKinds = this.indexedPathKinds();
    const uniquePaths = [...new Set(request.paths.map((path) => path.trim()).filter(Boolean))];
    return Promise.all(
      uniquePaths.map(async (candidate): Promise<PathResolution> => {
        const resolvedPath = resolveCandidatePath(candidate, request.relativeBase);
        const normalizedPath = normalizePath(resolvedPath);
        const indexedKind = indexedKinds.get(normalizedPath);
        if (indexedKind) {
          return { kind: indexedKind, path: candidate, resolvedPath };
        }

        const cachedKind = this.directPathCache.get(normalizedPath);
        if (cachedKind) {
          return { kind: cachedKind, path: candidate, resolvedPath };
        }

        let kind: PathResolution["kind"] = "missing";
        try {
          const stat = await vscode.workspace.fs.stat(vscode.Uri.file(resolvedPath));
          kind = stat.type & vscode.FileType.Directory ? "directory" : "file";
        } catch {
          // A failed stat must not become a clickable chip. Permission errors
          // are deliberately treated like missing paths: the safer UI is code.
        }
        this.directPathCache.set(normalizedPath, kind);
        return { kind, path: candidate, resolvedPath };
      }),
    );
  }

  async ensureCache(
    maxFiles = readContextSearchConfig().maxFiles,
    token?: vscode.CancellationToken,
  ): Promise<ContextSearchCache> {
    if (this.cache && !this.dirty && this.cache.maxFiles === maxFiles) {
      return this.cache;
    }

    // A create/delete during the async listing invalidates that snapshot. Rebuild
    // once for this query; under ongoing churn keep the cache dirty, not immortal.
    for (let pass = 0; ; pass += 1) {
      const revision = this.indexRevision;
      const files = await vscode.workspace.findFiles("**/*", undefined, maxFiles, token);
      const directories = deriveDirectories(files);
      const cache: ContextSearchCache = {
        candidates: [
          ...files.map((uri) => buildCachedCandidate(uri, false)),
          ...directories.map((uri) => buildCachedCandidate(uri, true)),
        ],
        maxFiles,
        truncated: files.length >= maxFiles,
      };
      const cancelled = token?.isCancellationRequested === true;
      const current = revision === this.indexRevision;
      if (!cancelled && current) {
        this.cache = cache;
        this.dirty = false;
      }
      if (cancelled || current || pass === 1) return cache;
    }
  }

  private indexedPathKinds(): Map<string, PathResolution["kind"]> {
    if (!this.cache || this.dirty) {
      return new Map();
    }
    return new Map(
      this.cache.candidates.map((candidate) => [
        candidate.normalizedFsPath,
        candidate.isDirectory ? "directory" : "file",
      ]),
    );
  }

  private markDirty(): void {
    this.indexRevision += 1;
    this.cache = null;
    this.directPathCache.clear();
    this.dirty = true;
  }
}
