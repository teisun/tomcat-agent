function deepseekAfterDate(freshness) {
  var days = { day: 1, week: 7, month: 30, year: 365 }[freshness];
  if (!days) return null;
  return new Date(Date.now() - days * 86400000).toISOString().slice(0, 10);
}

function parseDeepSeekResponse(body) {
  if (!body || !Array.isArray(body.results)) {
    throw new Error("DeepSeek search response is missing a results array");
  }
  return body.results.filter(function (item) {
    return item && typeof item.url === "string" && item.url.trim();
  }).map(function (item) {
    var snippet = typeof item.snippet === "string" ? item.snippet : "";
    var summary = typeof item.summary === "string" ? item.summary : "";
    return {
      title: typeof item.title === "string" && item.title.trim() ? item.title : item.url,
      url: item.url,
      snippet: snippet || summary || null,
      published_at: typeof item.date === "string" ? item.date : null
    };
  });
}

async function searchWithDeepSeek(req) {
  var warnings = req.country ? ["deepseek_ignores_country"] : [];
  var query = normalizeQuery(req.query).trim();
  if (query) {
    query = rewriteQueryWithDomainFilter(query, req.domainFilter);
    if (req.language) query += " lang:" + req.language;
    var after = deepseekAfterDate(req.freshness);
    if (after) query += " after:" + after;
  }
  if (!query || Array.from(query).length > 128) {
    return {
      backend: "deepseek",
      hits: [],
      warnings: warnings.concat([query ? "deepseek_query_too_long" : "deepseek_query_empty"]),
      unsupported_backend: true
    };
  }
  return fetchJsonBackend("deepseek", req, {
    secretName: "DEEPSEEK_API_KEY",
    warnings: warnings,
    statusMessages: { 402: "DeepSeek search: insufficient balance (HTTP 402)" },
    request: {
      method: "POST",
      url: "https://api.deepseek.com/v1/tools/search",
      headers: {
        Authorization: "Bearer {{secret:DEEPSEEK_API_KEY}}",
        "Content-Type": "application/json"
      },
      body: { query: query, count: normalizeCount(req.count), need_content: false }
    },
    parse: parseDeepSeekResponse
  });
}
