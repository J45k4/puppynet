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
var handleRoute = async (path) => {
  if (!matcher)
    return;
  const match = matcher.match(path);
  if (!match) {
    console.error("No route found for", path);
    return;
  }
  await Promise.resolve(match.result);
};
window.addEventListener("popstate", () => {
  handleRoute(window.location.pathname);
});
var routes = (routes2) => {
  matcher = patternMatcher(routes2);
  handleRoute(window.location.pathname);
};
var navigate = (path) => {
  window.history.pushState({}, "", path);
  handleRoute(path);
};

// src/api.ts
var envAddr = "http://localhost:4242";
var serverAddr = envAddr && envAddr.trim().length > 0 ? envAddr : typeof window !== "undefined" ? window.location.origin : "/";
var apiBase = serverAddr.endsWith("/") ? serverAddr.slice(0, -1) : serverAddr;
var peersCache = null;
var getServerAddr = () => serverAddr;
var apiGet = async (path) => {
  const res = await fetch(`${apiBase}${path}`);
  if (!res.ok) {
    throw new Error(`Request failed: ${res.status}`);
  }
  return res.json();
};
var fetchPeers = async () => {
  if (!peersCache) {
    const data = await apiGet("/api/peers");
    peersCache = data.peers;
  }
  return peersCache ?? [];
};
var findPeer = async (peerId) => {
  const peers = await fetchPeers();
  return peers.find((p) => p.id === peerId);
};
var fetchMimeTypes = async () => {
  const data = await apiGet("/api/mime-types");
  return data.mime_types;
};
var searchFiles = async (args) => {
  const params = new URLSearchParams;
  if (args.name_query)
    params.set("name_query", args.name_query);
  if (args.mime_types && args.mime_types.length > 0) {
    params.set("mime_types", args.mime_types.join(","));
  }
  if (args.page !== undefined)
    params.set("page", String(args.page));
  if (args.page_size !== undefined)
    params.set("page_size", String(args.page_size));
  const res = await fetch(`${apiBase}/api/search?${params.toString()}`);
  if (!res.ok) {
    throw new Error(`Search failed: ${res.status}`);
  }
  const data = await res.json();
  return {
    results: data.results ?? [],
    total: data.total ?? 0,
    mime_types: data.mime_types ?? []
  };
};

// src/layout.ts
var ensureShell = (currentPath) => {
  let root = document.getElementById("app-root");
  if (!root) {
    document.body.innerHTML = "";
    root = document.createElement("div");
    root.id = "app-root";
    document.body.appendChild(root);
  }
  root.innerHTML = `
<div class="page">
	<nav class="nav">
		<a href="/" data-route="/">Home</a>
		<a href="/peers" data-route="/peers">Peers</a>
		<a href="/files" data-route="/files">Files</a>
		<a href="/search" data-route="/search">Search</a>
		<a href="/storage" data-route="/storage">Storage</a>
		<a href="/updates" data-route="/updates">Updates</a>
		<a href="/settings" data-route="/settings">Settings</a>
	</nav>
	<main id="content"></main>
</div>
`;
  const navLinks = root.querySelectorAll(".nav a");
  navLinks.forEach((link) => {
    const route = link.getAttribute("data-route");
    if (route === currentPath || route && currentPath.startsWith(route + "/")) {
      link.classList.add("active");
    }
    link.addEventListener("click", (ev) => {
      ev.preventDefault();
      const href = link.getAttribute("href");
      if (href)
        navigate(href);
    });
  });
  const content = root.querySelector("#content");
  if (!content)
    throw new Error("content mount missing");
  return content;
};

// src/pages/home.ts
var renderHome = () => {
  const content = ensureShell("/");
  content.innerHTML = `
<section class="hero">
	<h1>PuppyNet</h1>
	<p class="lede">Welcome to PuppyNet. Use the navigation to explore peers, files, and settings.</p>
</section>
<div class="card">
	<h2>Getting started</h2>
	<p class="muted">Browse peers to inspect connections or open other sections to mirror the desktop GUI.</p>
</div>
`;
};

// src/pages/peers.ts
var renderPeers = async () => {
  const content = ensureShell("/peers");
  content.innerHTML = `
		<section class="hero">
			<h1>Peers</h1>
			<p class="lede">Connected peers discovered by PuppyNet.</p>
		</section>
		<div class="card" id="peers-card">
			<h2>Peer list</h2>
			<p id="peers-status" class="muted">Loading peers...</p>
			<div id="peers-table"></div>
		</div>
	`;
  const statusEl = document.getElementById("peers-status");
  const tableEl = document.getElementById("peers-table");
  try {
    const peers = await fetchPeers();
    if (statusEl)
      statusEl.textContent = `${peers.length} peer(s)`;
    if (!tableEl)
      return;
    if (peers.length === 0) {
      tableEl.innerHTML = `<p class="muted">No peers connected.</p>`;
      return;
    }
    const rows = peers.map((peer) => `
			<tr data-peer-id="${peer.id}">
				<td><div class="pill"><strong>${peer.name ?? "Unnamed"}</strong><span class="muted">${peer.id}</span></div></td>
				<td><button class="link-btn" data-peer-id="${peer.id}">Open</button></td>
			</tr>
		`).join("");
    tableEl.innerHTML = `
			<table class="table">
				<thead>
					<tr><th>Peer</th><th></th></tr>
				</thead>
				<tbody>${rows}</tbody>
			</table>
		`;
    const buttons = tableEl.querySelectorAll("[data-peer-id]");
    buttons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const id = btn.getAttribute("data-peer-id");
        if (id)
          navigate(`/peers/${id}`);
      });
    });
  } catch (err) {
    if (statusEl)
      statusEl.textContent = `Failed to load peers: ${err}`;
  }
};
var renderPeerDetail = async (peerId) => {
  const content = ensureShell("/peers");
  content.innerHTML = `
		<section class="hero">
			<h1>Peer</h1>
			<p class="lede">Details for peer ${peerId}</p>
		</section>
		<div class="card" id="peer-card">
			<h2>Summary</h2>
			<p id="peer-status" class="muted">Loading peer info...</p>
			<div id="peer-details"></div>
			<button class="link-btn" id="back-to-peers">Back to peers</button>
		</div>
	`;
  const backBtn = document.getElementById("back-to-peers");
  if (backBtn) {
    backBtn.addEventListener("click", () => navigate("/peers"));
  }
  try {
    const peer = await findPeer(peerId);
    if (!peer) {
      const statusEl2 = document.getElementById("peer-status");
      if (statusEl2)
        statusEl2.textContent = "Peer not found.";
      return;
    }
    const detailsEl = document.getElementById("peer-details");
    if (!detailsEl)
      return;
    const statusEl = document.getElementById("peer-status");
    if (statusEl)
      statusEl.textContent = "Connected peer loaded.";
    detailsEl.innerHTML = `
			<p><span class="muted">Name:</span> ${peer.name ?? "Unnamed"}</p>
			<p><span class="muted">Peer ID:</span> ${peer.id}</p>
		`;
  } catch (err) {
    const statusEl = document.getElementById("peer-status");
    if (statusEl)
      statusEl.textContent = `Failed to load peer: ${err}`;
  }
};

// src/pages/files.ts
var renderFiles = () => {
  const content = ensureShell("/files");
  content.innerHTML = `
		<section class="hero">
			<h1>Files</h1>
			<p class="lede">Coming soon: browse local and shared files.</p>
		</section>
		<div class="card"><p class="muted">The file browser UI from the GUI will be mirrored here.</p></div>
	`;
};

// src/multiselect.ts
var createMultiSelect = (props) => {
  const selected = new Set;
  let allOptions = props.options ?? [];
  const wrapper = document.createElement("div");
  wrapper.className = "multiselect";
  if (props.id)
    wrapper.id = props.id;
  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "multiselect-trigger";
  const label = document.createElement("span");
  label.textContent = props.placeholder ?? "Select...";
  const caret = document.createElement("span");
  caret.className = "multiselect-caret";
  caret.textContent = "▾";
  trigger.append(label, caret);
  const panel = document.createElement("div");
  panel.className = "multiselect-panel";
  const searchBox = document.createElement("input");
  searchBox.type = "text";
  searchBox.placeholder = "Search...";
  searchBox.className = "multiselect-search";
  const optionsEl = document.createElement("div");
  optionsEl.className = "multiselect-options";
  panel.append(searchBox, optionsEl);
  wrapper.append(trigger, panel);
  let open = false;
  const close = () => {
    if (!open)
      return;
    open = false;
    wrapper.classList.remove("open");
  };
  const toggle = () => {
    open = !open;
    if (open)
      wrapper.classList.add("open");
    else
      wrapper.classList.remove("open");
  };
  const updateLabel = () => {
    if (selected.size === 0) {
      label.textContent = props.placeholder ?? "Select...";
    } else if (selected.size <= 2) {
      label.textContent = Array.from(selected).join(", ");
    } else {
      label.textContent = `${selected.size} selected`;
    }
  };
  const notify = () => {
    updateLabel();
    if (props.onChange)
      props.onChange(Array.from(selected));
  };
  trigger.addEventListener("click", (ev) => {
    ev.stopPropagation();
    toggle();
  });
  document.addEventListener("click", (ev) => {
    if (!wrapper.contains(ev.target)) {
      close();
    }
  });
  const renderOptions = (options) => {
    optionsEl.innerHTML = "";
    options.forEach((opt) => {
      const row = document.createElement("label");
      row.className = "multiselect-option";
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.value = opt.value;
      checkbox.checked = selected.has(opt.value);
      const text = document.createElement("span");
      text.textContent = opt.label ?? opt.value;
      row.append(checkbox, text);
      checkbox.addEventListener("change", (ev) => {
        const target = ev.target;
        if (target.checked)
          selected.add(opt.value);
        else
          selected.delete(opt.value);
        notify();
      });
      row.addEventListener("click", (ev) => ev.stopPropagation());
      optionsEl.appendChild(row);
    });
  };
  const applyFilter = () => {
    const query = searchBox.value.trim().toLowerCase();
    if (!query) {
      renderOptions(allOptions);
      return;
    }
    renderOptions(allOptions.filter((opt) => {
      const label2 = opt.label ?? opt.value;
      return label2.toLowerCase().includes(query) || opt.value.toLowerCase().includes(query);
    }));
  };
  const setOptions = (options) => {
    allOptions = options;
    applyFilter();
    notify();
  };
  const setSelected = (values) => {
    selected.clear();
    values.forEach((v) => selected.add(v));
    optionsEl.querySelectorAll("input[type=checkbox]").forEach((cb) => {
      cb.checked = selected.has(cb.value);
    });
    notify();
  };
  searchBox.addEventListener("input", () => applyFilter());
  if (props.options) {
    setOptions(allOptions);
  } else {
    updateLabel();
  }
  return {
    element: wrapper,
    getSelected: () => Array.from(selected),
    setOptions,
    setSelected,
    clear: () => setSelected([])
  };
};

// src/pages/search.ts
var defaultPageSize = 25;
var renderSearch = async () => {
  const content = ensureShell("/search");
  content.innerHTML = `
		<section class="hero">
			<h1>Search</h1>
			<p class="lede">Search across indexed files.</p>
		</section>
		<div class="card">
			<h2>Filters</h2>
			<form id="search-form">
				<input id="search-name" name="name_query" placeholder="Name contains..." />
				<div id="search-mime"></div>
				<button type="submit">Search</button>
			</form>
			<p id="search-status" class="muted">Enter a query to search.</p>
		</div>
		<div class="card" id="search-results">
			<h2>Results</h2>
			<div id="search-table"></div>
		</div>
	`;
  const statusEl = document.getElementById("search-status");
  const tableEl = document.getElementById("search-table");
  const nameInput = document.getElementById("search-name");
  const mimeMount = document.getElementById("search-mime");
  const mimeSelect = createMultiSelect({
    id: "search-mime-select",
    placeholder: "Mime types"
  });
  if (mimeMount?.parentElement) {
    mimeMount.parentElement.replaceChild(mimeSelect.element, mimeMount);
  }
  let currentPage = 0;
  let totalResults = 0;
  let loading = false;
  let hasMore = false;
  let observer = null;
  const loadMimeTypes = async () => {
    try {
      const mimes = await fetchMimeTypes();
      mimeSelect.setOptions(mimes.map((m) => ({
        value: m,
        label: m
      })));
    } catch (err) {
      if (statusEl)
        statusEl.textContent = `Failed to load mime types: ${err}`;
    }
  };
  const resetTable = () => {
    if (!tableEl)
      return;
    tableEl.innerHTML = `
			<div class="table-wrapper">
				<table class="table">
					<thead>
						<tr>
							<th>Name</th>
							<th>Type</th>
						<th>Size</th>
						<th>Replicas</th>
						<th>Updated</th>
					</tr>
				</thead>
					<tbody id="search-body"></tbody>
				</table>
			</div>
			<div id="search-sentinel"></div>
		`;
  };
  const appendRows = (rows) => {
    const body = document.getElementById("search-body");
    if (!body)
      return;
    const html = rows.map((r) => `
					<tr>
						<td>${r.name}</td>
						<td class="muted">${r.mime_type ?? "unknown"}</td>
						<td>${((r.size ?? 0) / 1024).toFixed(1)} KB</td>
						<td><span class="badge small">${r.replicas} replicas</span></td>
						<td class="muted">${r.latest_datetime ?? ""}</td>
					</tr>
				`).join("");
    body.insertAdjacentHTML("beforeend", html);
  };
  const loadPage = async () => {
    if (loading)
      return;
    loading = true;
    if (statusEl)
      statusEl.textContent = "Searching...";
    try {
      const name_query = nameInput?.value.trim() ?? "";
      const mime_types = mimeSelect.getSelected();
      const data = await searchFiles({
        name_query: name_query || undefined,
        mime_types: mime_types.length ? mime_types : undefined,
        page: currentPage,
        page_size: defaultPageSize
      });
      totalResults = data.total ?? 0;
      if (!tableEl)
        return;
      if (currentPage === 0) {
        resetTable();
      }
      if (!data.results.length && currentPage === 0) {
        tableEl.innerHTML = `<p class="muted">No results.</p>`;
        hasMore = false;
        return;
      }
      appendRows(data.results);
      currentPage += 1;
      const loadedCount = Math.min(currentPage * defaultPageSize, totalResults);
      if (statusEl)
        statusEl.textContent = `Loaded ${loadedCount} of ${totalResults} result(s)`;
      hasMore = loadedCount < totalResults;
      const sentinel = document.getElementById("search-sentinel");
      if (sentinel) {
        if (!observer) {
          observer = new IntersectionObserver((entries) => {
            if (entries.some((e) => e.isIntersecting) && hasMore) {
              loadPage();
            }
          });
        }
        if (hasMore)
          observer.observe(sentinel);
        else
          observer.unobserve(sentinel);
      }
    } catch (err) {
      if (statusEl)
        statusEl.textContent = `Search failed: ${err}`;
    } finally {
      loading = false;
    }
  };
  const form = document.getElementById("search-form");
  form?.addEventListener("submit", (ev) => {
    ev.preventDefault();
    currentPage = 0;
    totalResults = 0;
    hasMore = false;
    if (observer) {
      const sentinel = document.getElementById("search-sentinel");
      if (sentinel)
        observer.unobserve(sentinel);
    }
    resetTable();
    loadPage();
  });
  loadMimeTypes();
};

// src/pages/storage.ts
var renderStorage = () => {
  const content = ensureShell("/storage");
  content.innerHTML = `
		<section class="hero">
			<h1>Storage</h1>
			<p class="lede">Storage overview and replication.</p>
		</section>
		<div class="card"><p class="muted">Storage dashboard placeholder.</p></div>
	`;
};

// src/pages/updates.ts
var renderUpdates = () => {
  const content = ensureShell("/updates");
  content.innerHTML = `
		<section class="hero">
			<h1>Updates</h1>
			<p class="lede">Manage updates and versions.</p>
		</section>
		<div class="card"><p class="muted">Update management placeholder.</p></div>
	`;
};

// src/pages/settings.ts
var renderSettings = () => {
  const content = ensureShell("/settings");
  content.innerHTML = `
		<section class="hero">
			<h1>Settings</h1>
			<p class="lede">Configure PuppyNet.</p>
		</section>
		<div class="card"><p class="muted">Settings UI placeholder.</p></div>
	`;
};

// src/app.ts
var serverAddr2 = getServerAddr();
window.onload = () => {
  const body = document.querySelector("body");
  if (!body) {
    throw new Error("No body element found");
  }
  routes({
    "/": () => renderHome(),
    "/peers": () => renderPeers(),
    "/peers/:peerId": ({ peerId }) => renderPeerDetail(peerId),
    "/files": () => renderFiles(),
    "/search": () => renderSearch(),
    "/storage": () => renderStorage(),
    "/updates": () => renderUpdates(),
    "/settings": () => renderSettings(),
    "/*": () => renderHome()
  });
  console.info("Using server address:", serverAddr2);
};
