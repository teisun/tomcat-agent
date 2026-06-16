var backends = {
  mimo: searchWithMimo,
  tavily: searchWithTavily,
  brave: searchWithBrave,
  serper: searchWithSerper
};

function nextAutoBackend(startIndex) {
  for (var i = startIndex + 1; i < autoOrder.length; i += 1) {
    if (typeof backends[autoOrder[i]] === "function") {
      return autoOrder[i];
    }
  }
  return null;
}

function isAutoRetryWarning(warning) {
  return typeof warning === "string" &&
    (warning.indexOf("__missing_key__:") === 0 || warning.indexOf("__unauthorized__:") === 0);
}

function translateAutoWarning(backend, warning) {
  if (typeof warning !== "string") {
    return "backend_unavailable:" + backend;
  }
  if (warning.indexOf("__missing_key__:") === 0) {
    var secretName = warning.slice("__missing_key__:".length);
    return "missing_key (backend=" + backend + ",secret=" + secretName + ")";
  }
  if (warning.indexOf("__unauthorized__:") === 0) {
    var status = warning.slice("__unauthorized__:".length) || "401";
    return "unauthorized (backend=" + backend + ",status=" + status + ")";
  }
  return warning;
}

function fallbackWarning(backend, nextBackend) {
  if (nextBackend) {
    return "backend_unavailable:" + backend + ", fallback=" + nextBackend;
  }
  return "backend_unavailable:" + backend;
}

function buildAutoFallbackWarnings(backend, nextBackend, responseWarnings, err) {
  var warnings = [fallbackWarning(backend, nextBackend)];
  var sourceWarnings = Array.isArray(responseWarnings) ? responseWarnings : [];
  for (var i = 0; i < sourceWarnings.length; i += 1) {
    warnings.push(translateAutoWarning(backend, sourceWarnings[i]));
  }
  if (err) {
    warnings.push("plugin_backend_error (backend=" + backend + "): " + String(err));
  }
  return warnings;
}

function shouldRetryAutoResponse(response) {
  if (!response || !Array.isArray(response.warnings)) {
    return false;
  }
  for (var i = 0; i < response.warnings.length; i += 1) {
    if (isAutoRetryWarning(response.warnings[i])) {
      return true;
    }
  }
  return false;
}

async function dispatchBackend(req) {
  var backend = req.backend || "auto";
  if (backend === "auto") {
    var warnings = [];
    for (var i = 0; i < autoOrder.length; i += 1) {
      var name = autoOrder[i];
      if (typeof backends[name] === "function") {
        var nextBackend = nextAutoBackend(i);
        try {
          var response = await backends[name](cloneReq(req, name));
          if (shouldRetryAutoResponse(response)) {
            warnings = warnings.concat(
              buildAutoFallbackWarnings(name, nextBackend, response.warnings, null)
            );
            continue;
          }
          response = response || {};
          response.warnings = warnings.concat(
            Array.isArray(response.warnings) ? response.warnings : []
          );
          return response;
        } catch (err) {
          warnings = warnings.concat(
            buildAutoFallbackWarnings(name, nextBackend, null, err)
          );
        }
      }
    }
    return {
      backend: "auto",
      hits: [],
      warnings: warnings,
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
