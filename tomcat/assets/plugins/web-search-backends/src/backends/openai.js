var OPENAI_SEARCH_MODEL = "gpt-6.2-sol";

function parseOpenAISearch(body) {
  if (!body || !Array.isArray(body.output) || body.error) {
    throw new Error((body && body.error && body.error.message) || "OpenAI web search returned no output");
  }
  var complete = body.status === undefined || body.status === "completed";
  var hits = [];
  var sources = [];
  for (var item of body.output) {
    if (!item) continue;
    if (item.type === "message") {
      for (var part of toArray(item.content)) {
        if (!part || part.type !== "output_text") continue;
        var chars = Array.from(part.text || "");
        for (var annotation of collectAnnotations(part)) {
          var hit = annotationToHit(annotation);
          if (!hit) continue;
          var start = annotation.start_index;
          var end = annotation.end_index;
          if (Number.isInteger(start) && Number.isInteger(end) && start >= 0 && start < end && end <= chars.length) {
            hit.snippet = chars.slice(start, end).join("");
          }
          hits.push(hit);
        }
      }
    } else if (item.type === "web_search_call") {
      if (item.status === "failed" || item.status === "incomplete") complete = false;
      for (var source of toArray(item.action && item.action.sources)) {
        if (source && typeof source.url === "string") {
          sources.push({ url: source.url, title: source.title || null, snippet: source.snippet || null });
        }
      }
    }
  }
  // Partial output still carries usable links; an empty unfinished one must let routing continue.
  hits = hits.concat(sources);
  if (!hits.length && !complete) {
    throw new Error("OpenAI web search did not complete");
  }
  return hits;
}

async function searchWithOpenAI(req) {
  var tool = { type: "web_search", search_context_size: "medium" };
  var domains = toArray(req.domainFilter);
  if (domains.length) tool.filters = { allowed_domains: domains.slice() };
  if (req.country) tool.user_location = { type: "approximate", country: String(req.country).toUpperCase() };
  return fetchJsonBackend("openai", req, {
    request: {
      url: "https://api.openai.com/v1/responses",
      method: "POST",
      headers: { Authorization: "Bearer {{secret:OPENAI_API_KEY}}", "Content-Type": "application/json" },
      body: {
        model: OPENAI_SEARCH_MODEL,
        input: normalizeQuery(req.query),
        store: false,
        tool_choice: "required",
        include: ["web_search_call.action.sources"],
        tools: [tool]
      }
    },
    parse: parseOpenAISearch
  });
}
