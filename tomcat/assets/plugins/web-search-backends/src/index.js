var backends = {
  mimo: searchWithMimo,
  tavily: searchWithTavily,
  brave: searchWithBrave,
  serper: searchWithSerper
};

async function dispatchBackend(req) {
  var backend = req.backend || "auto";
  if (backend === "auto") {
    for (var i = 0; i < autoOrder.length; i += 1) {
      var name = autoOrder[i];
      if (typeof backends[name] === "function") {
        return backends[name](cloneReq(req, name));
      }
    }
    return {
      backend: "auto",
      hits: [],
      warnings: [],
      unsupported_backend: true
    };
  }

  if (typeof backends[backend] !== "function") {
    return {
      backend: backend,
      hits: [],
      warnings: [],
      unsupported_backend: true
    };
  }

  return backends[backend](req);
}

pi.registerFunction("webSearchBackend", async function (params) {
  params = params || {};
  return dispatchBackend(params);
});
