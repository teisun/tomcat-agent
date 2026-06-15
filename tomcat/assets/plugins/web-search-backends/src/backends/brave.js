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
