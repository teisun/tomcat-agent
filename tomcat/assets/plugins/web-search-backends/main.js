var MIMO_MODEL = "mimo-v2.5-pro";
var autoOrder = ["mimo"];

function toArray(value) {
  return Array.isArray(value) ? value : [];
}

function buildWebSearchTool(req) {
  var tool = {
    type: "web_search",
    force_search: true,
    limit: Math.max(1, Math.min(Number(req.count || 5), 10)),
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
        content: String(req.query || "")
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

var backends = {
  mimo: searchWithMimo
};

async function dispatchBackend(req) {
  var backend = req.backend || "auto";
  if (backend === "auto") {
    for (var i = 0; i < autoOrder.length; i += 1) {
      var name = autoOrder[i];
      if (typeof backends[name] === "function") {
        return backends[name](Object.assign({}, req, { backend: name }));
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
