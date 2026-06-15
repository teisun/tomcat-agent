var MIMO_MODEL = "mimo-v2.5-pro";
var TAVILY_BASE_URL = "https://api.tavily.com";
var BRAVE_BASE_URL = "https://api.search.brave.com";
var SERPER_BASE_URL = "https://google.serper.dev";
var autoOrder = ["mimo", "tavily", "brave", "serper"];

function toArray(value) {
  return Array.isArray(value) ? value : [];
}

function cloneReq(req, backend) {
  return Object.assign({}, req, { backend: backend });
}

function normalizeBaseUrl(raw, fallback) {
  return String(raw || fallback).replace(/\/+$/, "");
}

function providerBaseUrl(req, key, fallback) {
  return normalizeBaseUrl(req && req[key], fallback);
}

function normalizeQuery(raw) {
  return String(raw || "");
}

function normalizeCount(raw) {
  return Math.max(1, Math.min(Number(raw || 5), 10));
}

function dedupeHits(hits) {
  var out = [];
  var seen = Object.create(null);
  for (var i = 0; i < hits.length; i += 1) {
    var hit = hits[i];
    if (!hit || !hit.url || seen[hit.url]) {
      continue;
    }
    seen[hit.url] = true;
    out.push(hit);
  }
  return out;
}

function sentinelResponse(backend, warning) {
  return {
    backend: backend,
    hits: [],
    warnings: [warning]
  };
}

function missingKeyWarning(secretName) {
  return "__missing_key__:" + String(secretName || "");
}

function unauthorizedWarning(status) {
  return "__unauthorized__:" + String(status || 401);
}

function isUnauthorizedStatus(status) {
  return status === 401 || status === 403;
}

function extractSecretName(err, fallback) {
  if (err && err.details && err.details.secretName) {
    return String(err.details.secretName);
  }
  return fallback;
}

function parseJsonBody(raw, backend) {
  var text = raw == null ? "" : String(raw);
  if (!text) {
    return {};
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new Error("web_search backend `" + backend + "` returned invalid JSON");
  }
}

async function fetchJsonBackend(backend, req, options) {
  try {
    var response = await pi.fetch(options.request);
    if (response && response.data && typeof response.data.status === "number") {
      response = response.data;
    }
    if (isUnauthorizedStatus(response && response.status)) {
      return sentinelResponse(backend, unauthorizedWarning(response.status));
    }
    if (!response || typeof response.status !== "number") {
      throw new Error(
        "web_search backend `" + backend + "` returned an invalid HTTP envelope: " +
        JSON.stringify(response)
      );
    }
    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        "web_search backend `" + backend + "` returned HTTP status " + response.status
      );
    }
    return {
      backend: backend,
      hits: dedupeHits(options.parse(parseJsonBody(response.body, backend))),
      warnings: options.warnings || []
    };
  } catch (err) {
    if (err && err.code === "missing_secret") {
      return {
        backend: backend,
        hits: [],
        warnings: (options.warnings || []).concat([
          missingKeyWarning(extractSecretName(err, options.secretName))
        ])
      };
    }
    throw err;
  }
}

function rewriteQueryWithDomainFilter(query, domains) {
  if (!domains || !domains.length) {
    return query;
  }
  var filters = [];
  for (var i = 0; i < domains.length; i += 1) {
    filters.push("site:" + domains[i]);
  }
  return "(" + query + ") (" + filters.join(" OR ") + ")";
}

function buildWebSearchTool(req) {
  var tool = {
    type: "web_search",
    force_search: true,
    limit: normalizeCount(req.count),
    max_keyword: 3
  };

  if (req.domainFilter && req.domainFilter.length) {
    tool.filters = {
      allowed_domains: req.domainFilter.slice()
    };
  }

  if (req.country) {
    tool.user_location = {
      type: "approximate",
      country: String(req.country).toUpperCase()
    };
  }

  return tool;
}

function collectAnnotations(message) {
  var annotations = [];
  if (message && Array.isArray(message.annotations)) {
    annotations = annotations.concat(message.annotations);
  }
  if (message && Array.isArray(message.content)) {
    for (var i = 0; i < message.content.length; i += 1) {
      var part = message.content[i];
      if (part && Array.isArray(part.annotations)) {
        annotations = annotations.concat(part.annotations);
      }
    }
  }
  return annotations;
}

function annotationToHit(annotation) {
  if (!annotation || annotation.type !== "url_citation" || !annotation.url) {
    return null;
  }
  return {
    title: annotation.title || null,
    url: String(annotation.url),
    snippet: annotation.summary || annotation.snippet || null,
    published_at: annotation.publish_time || annotation.published_at || null
  };
}

function parseTavilyResponse(body) {
  var results = toArray(body && body.results);
  var hits = [];
  for (var i = 0; i < results.length; i += 1) {
    var item = results[i];
    if (!item || !item.url) {
      continue;
    }
    hits.push({
      title: item.title || null,
      url: String(item.url),
      snippet: item.content || item.snippet || null,
      published_at: item.published_date || item.published_at || null
    });
  }
  return hits;
}

function parseBraveResponse(body) {
  var results = toArray(body && body.web && body.web.results);
  var hits = [];
  for (var i = 0; i < results.length; i += 1) {
    var item = results[i];
    if (!item || !item.url) {
      continue;
    }
    hits.push({
      title: item.title || null,
      url: String(item.url),
      snippet: item.description || item.snippet || null,
      published_at: item.age || item.page_age || null
    });
  }
  return hits;
}

function parseSerperResponse(body) {
  var results = toArray(body && body.organic);
  var hits = [];
  for (var i = 0; i < results.length; i += 1) {
    var item = results[i];
    if (!item || !item.link) {
      continue;
    }
    hits.push({
      title: item.title || null,
      url: String(item.link),
      snippet: item.snippet || null,
      published_at: item.date || null
    });
  }
  return hits;
}

function tavilyTimeRange(freshness) {
  return freshness ? String(freshness) : null;
}

function braveFreshness(freshness) {
  if (freshness === "day") return "pd";
  if (freshness === "week") return "pw";
  if (freshness === "month") return "pm";
  if (freshness === "year") return "py";
  return null;
}

function serperFreshness(freshness) {
  if (freshness === "day") return "qdr:d";
  if (freshness === "week") return "qdr:w";
  if (freshness === "month") return "qdr:m";
  if (freshness === "year") return "qdr:y";
  return null;
}

async function searchWithMimo(req) {
  var warnings = [];
  if (req.language) {
    warnings.push("mimo_ignores_language");
  }

  var response = await pi.createChatCompletion({
    model: MIMO_MODEL,
    messages: [
      {
        role: "user",
        content: normalizeQuery(req.query)
      }
    ],
    tools: [buildWebSearchTool(req)]
  });

  var choice = response && response.choices && response.choices[0] ? response.choices[0] : null;
  var message = choice && choice.message ? choice.message : {};
  var annotations = collectAnnotations(message);
  var hits = [];

  for (var i = 0; i < annotations.length; i += 1) {
    var hit = annotationToHit(annotations[i]);
    if (hit) {
      hits.push(hit);
    }
  }

  return {
    backend: "mimo",
    hits: dedupeHits(hits),
    warnings: warnings
  };
}

async function searchWithTavily(req) {
  var warnings = [];
  if (req.country || req.language) {
    warnings.push("tavily_ignores_country_language");
  }
  var body = {
    query: normalizeQuery(req.query),
    max_results: normalizeCount(req.count)
  };
  if (req.freshness) {
    body.time_range = tavilyTimeRange(req.freshness);
  }
  if (req.domainFilter && req.domainFilter.length) {
    body.include_domains = req.domainFilter.slice();
  }
  return fetchJsonBackend("tavily", req, {
    secretName: "TAVILY_API_KEY",
    warnings: warnings,
    request: {
      method: "POST",
      url: providerBaseUrl(req, "tavilyBaseUrl", TAVILY_BASE_URL) + "/search",
      headers: {
        Authorization: "Bearer {{secret:TAVILY_API_KEY}}",
        "Content-Type": "application/json"
      },
      body: body
    },
    parse: parseTavilyResponse
  });
}

async function searchWithBrave(req) {
  var warnings = [];
  var query = normalizeQuery(req.query);
  if (req.domainFilter && req.domainFilter.length) {
    query = rewriteQueryWithDomainFilter(query, req.domainFilter);
    warnings.push("brave_domain_filter_via_query_rewrite");
  }
  var queryParams = {
    q: query,
    count: normalizeCount(req.count)
  };
  if (req.country) {
    queryParams.country = String(req.country);
  }
  if (req.language) {
    queryParams.search_lang = String(req.language);
  }
  if (req.freshness) {
    var mappedFreshness = braveFreshness(req.freshness);
    if (mappedFreshness) {
      queryParams.freshness = mappedFreshness;
    }
  }
  return fetchJsonBackend("brave", req, {
    secretName: "BRAVE_API_KEY",
    warnings: warnings,
    request: {
      method: "GET",
      url: providerBaseUrl(req, "braveBaseUrl", BRAVE_BASE_URL) + "/res/v1/web/search",
      headers: {
        Accept: "application/json",
        "X-Subscription-Token": "{{secret:BRAVE_API_KEY}}"
      },
      query: queryParams
    },
    parse: parseBraveResponse
  });
}

async function searchWithSerper(req) {
  var warnings = [];
  var query = normalizeQuery(req.query);
  if (req.domainFilter && req.domainFilter.length) {
    query = rewriteQueryWithDomainFilter(query, req.domainFilter);
    warnings.push("serper_domain_filter_via_query_rewrite");
  }
  var body = {
    q: query,
    num: normalizeCount(req.count)
  };
  if (req.country) {
    body.gl = String(req.country);
  }
  if (req.language) {
    body.hl = String(req.language);
  }
  if (req.freshness) {
    var mappedFreshness = serperFreshness(req.freshness);
    if (mappedFreshness) {
      body.tbs = mappedFreshness;
    }
  }
  return fetchJsonBackend("serper", req, {
    secretName: "SERPER_API_KEY",
    warnings: warnings,
    request: {
      method: "POST",
      url: providerBaseUrl(req, "serperBaseUrl", SERPER_BASE_URL) + "/search",
      headers: {
        "Content-Type": "application/json",
        "X-API-KEY": "{{secret:SERPER_API_KEY}}"
      },
      body: body
    },
    parse: parseSerperResponse
  });
}

var backends = {
  mimo: searchWithMimo,
  tavily: searchWithTavily,
  brave: searchWithBrave,
  serper: searchWithSerper
};

async function dispatchBackend(req) {
  var backend = req.backend || "auto";
  if (backend === "auto") {
    for (var i = 0; i < autoOrder.length; i += 1) {
      var name = autoOrder[i];
      if (typeof backends[name] === "function") {
        return backends[name](cloneReq(req, name));
      }
    }
    return {
      backend: "auto",
      hits: [],
      warnings: [],
      unsupported_backend: true
    };
  }

  if (typeof backends[backend] !== "function") {
    return {
      backend: backend,
      hits: [],
      warnings: [],
      unsupported_backend: true
    };
  }

  return backends[backend](req);
}

pi.registerFunction("webSearchBackend", async function (params) {
  params = params || {};
  return dispatchBackend(params);
});
