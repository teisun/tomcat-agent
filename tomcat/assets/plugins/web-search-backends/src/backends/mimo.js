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
