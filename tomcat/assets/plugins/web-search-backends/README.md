# web-search-backends

Official host-function plugin that provides `web_search.backend` providers for Tomcat.

## Runtime contract

- Request: `{ backend, query, count, freshness, country, language, domainFilter }`
- Response: `{ backend, hits, warnings, unsupported_backend? }`
- Installed into `~/.tomcat/plugins/web-search-backends/` by `tomcat init` when missing
- Existing user-edited runtime files are never overwritten by `tomcat init`

## Current backends

- `mimo`: uses `pi.createChatCompletion()` with model `mimo-v2.5-pro`
- `tavily`: uses `pi.fetch()` against `https://api.tavily.com/search`
- `brave`: uses `pi.fetch()` against `https://api.search.brave.com/res/v1/web/search`
- `serper`: uses `pi.fetch()` against `https://google.serper.dev/search`

## Auto order

```json
["mimo", "tavily", "brave", "serper"]
```

The plugin owns this order. Host-side `backend=auto` hands control to the plugin slot, and the
plugin decides which provider is tried first.

## Source layout

`main.js` is a generated runtime artifact. Edit the files under `src/` instead:

```text
web-search-backends/
├── plugin.json
├── README.md
├── main.js          # generated artifact consumed at runtime
└── src/
    ├── config.js
    ├── shared.js
    ├── parsers.js
    ├── index.js
    └── backends/
        ├── mimo.js
        ├── tavily.js
        ├── brave.js
        └── serper.js
```

## Build workflow

For the official builtin plugin in this repository:

- `npm run build:web-search-backends`
- or `cargo run --bin tomcat -- plugin build assets/plugins/web-search-backends`

For third-party plugins:

- prepare `<plugin-dir>/plugin.json`
- place authoring sources under `<plugin-dir>/src/`
- run `tomcat plugin build <plugin-dir>`
- ship the generated `main.js` together with `plugin.json`

The runtime still only loads `plugin.json.main` (typically `main.js`). It does not load `src/`
directly and does not resolve relative ES module imports at runtime.

## Authoring notes

- `pi` is injected by the host at runtime as `globalThis.pi`
- plugin source should **not** import `pi_bridge.js`
- IDE hints come from `tomcat/assets/types/tomcat-plugin.d.ts`
- an example TypeScript plugin lives at `tomcat/assets/plugins/examples/hello-plugin.ts`

## Runtime requirements

- manifest `requiredPermissions` includes `net:fetch`
- manifest `requiredSecrets` includes `TAVILY_API_KEY`, `BRAVE_API_KEY`, `SERPER_API_KEY`
- manifest `allowedHosts` includes `api.tavily.com`, `api.search.brave.com`, `google.serper.dev`

## `pi.fetch` safety model

- only `headers` and `body` values may contain `{{secret:NAME}}`
- `url` and query string must not contain secrets
- host must be in `allowedHosts`
- only HTTPS is allowed
- redirects are rejected by default
- SSRF checks reject private, loopback, link-local, IP-literal, and single-label hosts

## Migration note

- `backend=auto` now hands control to this plugin when hosted search is unavailable
- `backend=tavily|brave|serper|mimo` all resolve here by default
- `tools.web_search.legacy_http_backends = true` keeps the old Rust HTTP path available as a rollback switch
