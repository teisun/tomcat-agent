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
