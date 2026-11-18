# Official web search backends

Tomcat prepares this plugin at startup, including a fresh VS Code installation. `tomcat init` is not required. The managed copy lives at `<work_dir>/plugins/web-search-backends/`; Tomcat replaces changed bundle files there. Put custom copies in the project or agent plugin directory instead. Agent-layer copies can use saved keys; project-layer copies cannot, even if they use the same plugin ID.

## Choose where searches go

Open **Settings → Tools → Web Search**. Enable the providers you want and put your preferred provider first. Tomcat tries enabled providers in order, skips missing credentials, and stops at the first successful response, even if it has no results. Enable just one to use it exclusively. The default order is OpenAI hosted search (when configured), Tavily, Brave, Serper, MiMo, then DeepSeek. DeepSeek is skipped without a key. An existing saved list is not changed; if DeepSeek is off in that list, turn it on to use it.

DeepSeek uses the [official Search API](https://api-docs.deepseek.com/zh-cn/guides/search), separate from the active conversation model. Access and charges are governed by the DeepSeek API account, not inferred from a chat subscription. The configuration only stores provider IDs; keys use the same private store as model keys.

## Add or remove a provider

1. Implement one provider in `src/backends/<id>.js` and register it in `src/index.js`.
2. Declare it in `plugin.json` at `functions[].metadata.backends` for the `web_search.backend` point. Use `id`, `label`, and either a catalog `model` or a `secret` slot. For REST providers declare `host`, and add the slot and host to `requiredSecrets` and `allowedHosts`. Set `defaultAuto` only if the provider should participate for users without a saved list.
3. Rebuild `main.js` with `npm run build:web-search-backends` (or `tomcat plugin build assets/plugins/web-search-backends`). Test the bundle and rebuild the CLI to embed it.
4. To remove a provider, reverse the source and manifest changes and rebuild. Unknown IDs left in users’ saved lists are skipped, not redirected to another plugin.

The plugin handles one call `{ backend, model?, query, count, freshness, country, language, domainFilter }` and returns `{ hits, warnings, unsupported_backend? }`. Model-based providers use the supplied model, resolved by Tomcat. REST providers use `tomcat.fetch` with `{{secret:NAME}}` in a credential header. The host expands placeholders only in header values and leaves request bodies unchanged. Managed and agent-layer plugins may use saved keys at their declared HTTPS hosts; project or explicitly loaded plugins can only use explicit CLI environment variables. Shared Serve does not use inherited environment variables for credentials. The plugin does not route, retry other providers, or overwrite a user’s choice.
