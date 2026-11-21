# Official web search backends

Tomcat prepares this plugin at startup, including a fresh VS Code installation. `tomcat init` is not required. The managed copy lives at `<work_dir>/plugins/web-search-backends/`; Tomcat replaces changed bundle files there. Put custom copies in the project or agent plugin directory instead. Agent-layer copies can use saved keys; project-layer copies cannot, even if they use the same plugin ID.

## Choose where searches go

Open **Settings → Tools → Web Search**. Enable the providers you want and put your preferred provider first. Tomcat tries enabled providers in order, skips missing credentials, and stops at the first successful response, even if it has no results. Enable just one to use it exclusively. The default order is DeepSeek → Tavily → Brave → Serper → MiMo → OpenAI. A saved list is not changed. OpenAI uses `gpt-6.2-sol` through `https://api.openai.com/v1/responses` with `OPENAI_API_KEY`, without sending chat history. It no longer sends searches to model gateways or uses the legacy model `web_search` flag.

A search key is withheld when a model at another address uses the same key name, for example a gateway model configured with `OPENAI_API_KEY`. Settings shows this as a conflict; give that model its own key name in Models.

DeepSeek uses the [official Search API](https://api-docs.deepseek.com/zh-cn/guides/search), separate from the active conversation model. Access and charges are governed by the DeepSeek API account, not inferred from a chat subscription. The configuration only stores provider IDs; keys use the same private store as model keys.

## Add or remove a provider

1. Implement one provider in `src/backends/<id>.js` and register it in `src/index.js`.
2. Declare it in `plugin.json` at `functions[].metadata.backends` for the `web_search.backend` point. Use `id`, `label`, and either a catalog `model` or a `secret` slot with the HTTPS `host` that receives it. For REST providers add the `host` to `allowedHosts` and the `secret` to `requiredSecrets`. Set `defaultAuto` only if the provider should participate for users without a saved list.
3. Rebuild `main.js` with `npm run build:web-search-backends` (or `tomcat plugin build assets/plugins/web-search-backends`). Test the bundle and rebuild the CLI to embed it.
4. To remove a provider, reverse the source and manifest changes and rebuild. Unknown IDs left in users’ saved lists are skipped, not redirected to another plugin.

The plugin handles one call `{ backend, model?, query, count, freshness, country, language, domainFilter }` and returns `{ hits, warnings }`, or throws an `Error` whose message explains the failure. Tomcat reports that message as the backend's failure: with one enabled provider it is the tool error, with several it is recorded and the next provider is tried. Model-based providers use the supplied model, resolved by Tomcat. REST providers use `tomcat.fetch` with `{{secret:NAME}}` in a credential header; the host expands placeholders only in header values and rejects them elsewhere in the request. Managed and agent-layer plugins may use saved keys at their declared HTTPS hosts; project or explicitly loaded plugins can only use explicit CLI environment variables. Shared Serve does not use inherited environment variables for credentials. The plugin does not route, retry other providers, or overwrite a user’s choice.
