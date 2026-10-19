# Changelog

## 0.1.95

- Publish the Windows x64 support prepared for 0.1.94, with a bundled `tomcat.exe`, background Serve startup without a console window, Windows-aware `@` folder references, and process-tree cleanup for background commands and MCP servers. Install Git for Windows first: commands use Git Bash, never WSL. Windows ARM64 remains unsupported.
- Fix HOME expansion on macOS and other Unix systems so a backslash in a filename is preserved rather than silently pointing to a different file.
- Keep newly written credential files readable by their owner under restrictive Unix `umask` settings while retaining private permissions before any credential bytes are written. Existing credential files are not automatically repaired; Windows private access-list enforcement is unchanged.
- Reduce Git process overhead and polling delays during checkpoint creation when editing or resending earlier messages. On macOS, stopping an already-exited process group no longer reports a spurious permission error.
- Bundle CLI 0.1.79 in platform-specific packages. Pure-extension users must update their external CLI to receive these fixes.

## 0.1.94

- Add Windows x64 (`win32-x64`): the Marketplace, OpenVSX and GitHub packages bundle `tomcat.exe` (CLI 0.1.78). Commands run through Git Bash from Git for Windows, which must be installed; WSL's `bash.exe` is never used. When Git Bash is missing, the command fails with installation instructions instead of falling back.
- Start the Serve process without a console window, and resolve `@` folder references from Windows paths.
- On Windows, a background command still running when the CLI exits is stopped together with its process tree instead of blocking the exit, and processes detached by commands no longer hold the CLI's output pipes open.
- On Windows, MCP stdio servers launched through `npx` (such as the default Playwright server) are stopped with their whole process tree when the CLI exits or the connection closes, so exiting no longer waits for a browser call in progress.
- Show Windows paths in `tomcat init` and `tomcat doctor` with `\` separators only, instead of mixing `/` and `\`.
- On Windows, restrict `.env` and MCP OAuth token files to the current user by replacing their whole access list, so access granted to other accounts earlier is removed as well.
- Windows ARM64 and other platforms remain outside the support claim.

## 0.1.93

- Bring the default-model and utility-model selectors together at the top of Settings. Fix dropdown-arrow clicks and close the selector on Tab or Shift+Tab without disrupting keyboard navigation.
- Fix reopening the wrong session after switching back to an already-open conversation and reloading VS Code. Persist the latest explicit session selection for the current agent/project scope; separate windows still share that scope's saved selection.
- Close the session picker after choosing a session, including the current one, or when focus leaves the chat Webview. Return focus to the trigger after a selection without stealing focus on outside clicks. Keep pinning, list expansion and delete-confirmation operations open for continuous management.
- Bundle CLI 0.1.77 in platform-specific packages. Pure-extension users need to update their external CLI to receive the session-restore fix.

## 0.1.92

- Organize Settings into provider cards with one card per API key slot. Add **Connect provider**, remote model discovery, per-model protocol selection, and provider-level key replacement and deletion. Select the first configured model as the default only when no default is already set.
- Add a configurable utility model for session titles, turn titles and command descriptions. Refresh the built-in model catalog while preserving existing user model entries.
- Simplify model speed choices to Standard and Fast, including Anthropic Fast requests where supported. Read saved Ultrafast preferences as Standard without rewriting the user's files.
- Start or resume saved plans from an explicit implementation request, with confirmation in PLAN mode. Allow necessary questions during execution and continue the same turn after an answer; preserve the active plan binding while working with other plans.
- Keep permission-denied writes labelled as failed after history reload. Show older records without a stored outcome neutrally instead of claiming a file was created; this fixes the denied-write presentation issue documented in 0.1.90 for newly recorded results.
- Give selected question answers a distinct highlight, separate from hover, including high-contrast themes.
- Keep the session picker open during pin/unpin, list expansion and delete-confirmation operations.
- Bundle CLI 0.1.76 with a leaner built-in skill payload that excludes development dependencies. Unknown or removed configuration keys now produce a startup warning instead of blocking connection; invalid values for supported keys still fail validation.
- Publish platform-specific bundles to VS Code Marketplace for macOS Apple Silicon, macOS Intel and Linux x64, while retaining a CLI-free VSIX on GitHub.

## 0.1.90

- Keep Serve, history and Settings available without model credentials. Guide first-time users in the chat panel and enable sending after configuring a model, without a terminal initialization flow or Serve restart.
- Add default-model indicators and shortcuts, preserve form input on partial saves, and show concise inline connection errors with retry. Bundle CLI 0.1.74, including compaction fallback to the current main model.
- Publish this version to GitHub only. Marketplace publishing is independent and requires explicit opt-in after its OIDC configuration is ready; a GitHub release does not imply a Marketplace update.
- The release owner explicitly retained the known CLI 0.1.74 presentation issue: permission-refused writes may display Created after history reload even though no file was created. Native acceptance must still prove actual denial and file absence; the exception does not cover missing history status or future CLI versions.

## 0.1.89

- Change the extension display name from Tomcat to Tomcat AI Agent after Marketplace rejected the original display name as taken. Keep the extension ID `tomcat-agent.tomcat-vscode-ext`, commands, settings and Agent Box branding unchanged.
- Keep CLI 0.1.73 and the previously disclosed denied-write history-label limitation unchanged. Prepare new packages without replacing the published GitHub 0.1.88 assets; Marketplace availability remains subject to upload validation.

## 0.1.88

- Correct the minimum VS Code version to 1.106.0, where Secondary Side Bar view containers are supported by the stable extension API; older VS Code versions must be upgraded.
- Open the lowercase README shipped by vsce in the existing guide action, while retaining source-checkout support and reporting file-access failures.
- Prepare the extension's Marketplace identity as `tomcat-agent.tomcat-vscode-ext`; align installed-extension tests with the publisher.
- Update English and Chinese installation instructions, screenshot/documentation links, and model-service cost/data guidance. Retain GitHub VSIX downloads for offline installation.
- Keep the bundled Tomcat CLI pinned to 0.1.73. Marketplace targets are darwin-arm64, darwin-x64 and linux-x64; the pure extension remains a GitHub download, not a universal Marketplace fallback.
- Known issue retained with bundled CLI 0.1.73: permission-refused writes can be incorrectly labelled Created after history reload. The write remains refused and the file is not created. The release owner deferred this presentation repair; native acceptance reports it explicitly rather than claiming it was fixed.

## 0.1.14

- Fix the newly sent prompt drifting back down instead of staying pinned to the top ("reveal-to-top") in the real webview. The true trigger was a viewport-height change: when `busy` flips true the composer grows, so the stream container's `clientHeight` increases mid-reveal. The reveal spacer was sized for the old (smaller) viewport and never recomputed, so the scroll clamped to the bottom and the resulting scroll event flipped the mode into bottom-follow — abandoning the pin. The reveal now survives `clientHeight` changes: the `ResizeObserver` re-pins (recomputing/growing the spacer) on any viewport resize while revealing, and a resize-induced clamp to the bottom re-pins instead of falling into bottom-follow as long as the latest turn still fits. This path was invisible to jsdom tests, which mock `clientHeight` as a constant; new regression tests drive a changing viewport height.

## 0.1.13

- Fix the newly sent prompt not scrolling to the top of the transcript ("reveal-to-top") in the real webview. The reveal trigger and the session-reset handler previously lived in two `useLayoutEffect`s that shared the same tracking ref, so a `resetKey` change landing in the same commit as a new user message could silently eat the reveal — a timing race invisible to jsdom unit tests. Both are now merged into a single deterministic effect keyed on `resetKey / latestUserMessageId / oldestItemKey / userMessageCount`, so a session load/switch is authoritative (lands at the bottom) while an in-conversation send always reveals to the top.

## 0.1.5

- Fix **Add Model** official presets after `tomcat init`: seeded factory models now stay classified as built-in by the embedded preset list instead of being misread as user-only, so the official `Provider` picker is populated again in both the GUI and CLI `source` output.
- Replace the add-model segmented control with accessible tabs, default the empty-preset case to `Relay / custom endpoint`, show a reason beside disabled saves, and keep official edit backfill working even when a saved model overrides `base_url`.
- Seeded official models that live in `models.toml` now hide the GUI `Delete` button because they are correctly marked as built-in again; deleting those file-backed overrides still falls back to the factory preset behavior underneath.
- Change relay `thinking_format = Auto` to follow the selected API wire instead of guessing from `model_name`; older user-defined models without an explicit thinking format now use the wire-native encoding, with no migration required.

## 0.1.4

- Add the in-product **Add Models** flow: the composer model picker can open a dedicated settings center, and the extension now contributes `Tomcat: Open Settings`.
- Add model-management support across `tomcat serve --stdio`, the VS Code host, and the React webviews, including secure API key writes that stay in `.env` instead of being echoed back to the UI.
- Expand the built-in preset catalog and add Anthropic Messages support so OpenAI, DeepSeek, MiMo, GLM, Kimi, and Claude Opus presets can be configured from the same model-management surface.

## 0.1.3

- Refresh the README install examples and release filenames for the `0.1.3` VSIX line.
- Add bilingual extension documentation links so GitHub and bundled VSIX users can switch between English and Simplified Chinese.
- Clarify that the primary entry point is Tomcat Agent Box in the VS Code sidebar.

## 0.1.2

- Add platform-specific bundled VSIX packages that include the matching Tomcat CLI.
- Keep a pure extension VSIX for users who already have the CLI on their PATH.
- Improve first-run setup guidance so VS Code can guide users into `tomcat init`.
- Accept one known release risk: the Intel macOS (`darwin-x64`) bundle is cross-built in CI but not executed on an Intel mac during CI, so a manual Intel Mac spot-check is still recommended before broad rollout.
