# Changelog

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
