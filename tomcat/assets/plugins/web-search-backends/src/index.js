var backends = Object.assign(Object.create(null), {
  mimo: searchWithMimo,
  tavily: searchWithTavily,
  brave: searchWithBrave,
  serper: searchWithSerper,
  deepseek: searchWithDeepSeek,
  openai: searchWithOpenAI
});

async function dispatchBackend(req) {
  var backend = req.backend;
  if (typeof backends[backend] !== "function") {
    throw new Error("Unknown web_search backend: " + backend);
  }
  return backends[backend](req);
}

tomcat.registerFunction("webSearchBackend", async function (params) {
  return dispatchBackend(params || {});
});
