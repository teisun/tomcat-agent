# Changelog

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
