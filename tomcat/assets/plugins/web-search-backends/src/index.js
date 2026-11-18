var backends = Object.assign(Object.create(null), {
  mimo: searchWithMimo,
  tavily: searchWithTavily,
  brave: searchWithBrave,
  serper: searchWithSerper,
  deepseek: searchWithDeepSeek
});

async function dispatchBackend(req) {
  var backend = req.backend;
  if (typeof backends[backend] !== "function") {
    return { backend: backend, hits: [], warnings: [], unsupported_backend: true };
  }
  return backends[backend](req);
}

tomcat.registerFunction("webSearchBackend", async function (params) {
  return dispatchBackend(params || {});
});
