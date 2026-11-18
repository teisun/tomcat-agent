async function searchWithMimo(req) {
  if (typeof req.model !== "string" || !req.model) {
    return { backend: "mimo", hits: [], warnings: ["mimo_requires_model"], unsupported_backend: true };
  }
  var warnings = [];
  if (req.language) {
    warnings.push("mimo_ignores_language");
  }

  var response = await tomcat.createChatCompletion({
    model: req.model,
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
