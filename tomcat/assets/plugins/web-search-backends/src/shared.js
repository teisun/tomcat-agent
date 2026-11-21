function toArray(value) {
  return Array.isArray(value) ? value : [];
}

function normalizeQuery(raw) {
  return String(raw || "");
}

function normalizeCount(raw) {
  return Math.max(1, Math.min(Number(raw || 5), 20));
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
  var response = await tomcat.fetch(options.request);
  if (response && response.data && typeof response.data.status === "number") {
    response = response.data;
  }
  if (!response || typeof response.status !== "number") {
    throw new Error(
      "web_search backend `" + backend + "` returned an invalid HTTP envelope"
    );
  }
  if (response.status < 200 || response.status >= 300) {
    var detail;
    try {
      var payload = JSON.parse(response.body);
      if (payload && payload.error && typeof payload.error.message === "string") {
        detail = payload.error.message.replace(/[\r\n]+/g, " ").slice(0, 512);
      }
    } catch (_) { /* Non-JSON errors retain the HTTP status. */ }
    var message = (options.statusMessages && options.statusMessages[response.status]) ||
      "web_search backend `" + backend + "` returned HTTP status " + response.status;
    throw new Error(detail ? message + ": " + detail : message);
  }
  return {
    backend: backend,
    hits: dedupeHits(options.parse(parseJsonBody(response.body, backend))),
    warnings: options.warnings || []
  };
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
