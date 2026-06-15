# web-search-backends

Official host function plugin that provides `web_search.backend` providers for Tomcat.

Current backends:

- `mimo`: uses `pi.createChatCompletion()` with model `mimo-v2.5-pro`
- `tavily`: uses `pi.fetch()` against `https://api.tavily.com/search`
- `brave`: uses `pi.fetch()` against `https://api.search.brave.com/res/v1/web/search`
- `serper`: uses `pi.fetch()` against `https://google.serper.dev/search`

Auto order:

- `["mimo", "tavily", "brave", "serper"]`

Contract:

- request: `{ backend, query, count, freshness, country, language, domainFilter }`
- response: `{ backend, hits, warnings, unsupported_backend? }`

This plugin is installed into `~/.tomcat/plugins/web-search-backends/` by `tomcat init`
when missing. Existing user-edited files are never overwritten.

Runtime requirements:

- manifest `requiredPermissions` includes `net:fetch`
- manifest `requiredSecrets` includes `TAVILY_API_KEY`, `BRAVE_API_KEY`, `SERPER_API_KEY`
- manifest `allowedHosts` includes `api.tavily.com`, `api.search.brave.com`, `google.serper.dev`

`pi.fetch` safety model:

- only `headers` and `body` values may contain `{{secret:NAME}}`
- `url` and query string must not contain secrets
- host must be in `allowedHosts`
- only HTTPS is allowed
- redirects are rejected by default
- SSRF checks reject private, loopback, link-local, IP-literal, and single-label hosts

Migration note:

- `backend=auto` now hands control to this plugin when hosted search is unavailable
- `backend=tavily|brave|serper|mimo` all resolve here by default
- `tools.web_search.legacy_http_backends = true` keeps the old Rust HTTP path available as a rollback switch
