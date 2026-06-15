# web-search-backends

Official host function plugin that provides `web_search.backend` providers for Tomcat.

Current backends:

- `mimo`: uses `pi.createChatCompletion()` with model `mimo-v2.5-pro`

Contract:

- request: `{ backend, query, count, freshness, country, language, domainFilter }`
- response: `{ backend, hits, warnings, unsupported_backend? }`

This plugin is installed into `~/.tomcat/plugins/web-search-backends/` by `tomcat init`
when missing. Existing user-edited files are never overwritten.
