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
