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
