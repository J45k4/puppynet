// src/pattern-matcher.ts
function patternMatcher(handlers) {
  const typedHandlers = handlers;
  const routes = Object.keys(typedHandlers).sort((a, b) => {
    if (!a.includes("*") && !a.includes(":"))
      return -1;
    if (!b.includes("*") && !b.includes(":"))
      return 1;
    if (a.includes(":") && !b.includes(":"))
      return -1;
    if (!a.includes(":") && b.includes(":"))
      return 1;
    if (a.includes("*") && !b.includes("*"))
      return 1;
    if (!a.includes("*") && b.includes("*"))
      return -1;
    return b.length - a.length;
  });
  return {
    match(path) {
      for (const route of routes) {
        const params = matchRoute(route, path);
        if (params !== null) {
          const result = typedHandlers[route](params);
          return { pattern: route, result };
        }
      }
      return null;
    }
  };
}
function matchRoute(pattern, path) {
  const patternParts = pattern.split("/").filter((segment) => segment.length > 0);
  const pathParts = path.split("/").filter((segment) => segment.length > 0);
  if (pattern === "/*")
    return {};
  if (patternParts.length !== pathParts.length) {
    const lastPattern = patternParts[patternParts.length - 1];
    if (lastPattern === "*" && pathParts.length >= patternParts.length - 1) {
      return {};
    }
    return null;
  }
  const params = {};
  for (let i = 0;i < patternParts.length; i++) {
    const patternPart = patternParts[i];
    const pathPart = pathParts[i];
    if (patternPart === "*") {
      return params;
    }
    if (patternPart.startsWith(":")) {
      const paramName = patternPart.slice(1);
      params[paramName] = pathPart;
    } else if (patternPart !== pathPart) {
      return null;
    }
  }
  return params;
}

// src/router.ts
var matcher;
var handleRoute = (path) => {
  if (!matcher)
    return;
  const m = matcher.match(path);
  if (!m)
    console.error("No route found for", path);
  console.log("match result", m);
};
window.addEventListener("popstate", () => {
  handleRoute(window.location.pathname);
});
var routes = (routes2) => {
  matcher = patternMatcher(routes2);
  handleRoute(window.location.pathname);
};

// src/app.ts
var serverAddr = typeof process !== "undefined" && process.env.PUBLIC_SERVER_ADDR ? process.env.PUBLIC_SERVER_ADDR : "/";
window.onload = () => {
  const body = document.querySelector("body");
  if (!body) {
    throw new Error("No body element found");
  }
  routes({
    "/peers": () => document.body.innerHTML = "<h1>Peers</h1><p>List of peers will be shown here.</p>",
    "/*": () => document.body.innerHTML = "<h1>Home</h1><p>Welcome to PuppyNet!</p>"
  });
  console.info("Using server address:", serverAddr);
};
