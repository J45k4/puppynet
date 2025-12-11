// src/pattern-matcher.ts
function patternMatcher(handlers) {
  const routes = Object.keys(handlers).sort((a, b) => {
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
          const handler = handlers[route];
          if (!handler)
            continue;
          return {
            pattern: route,
            result: handler(params)
          };
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
    const lastPattern = patternParts[patternParts.length - 1] ?? "";
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

// src/api.ts
var envAddr = "http://localhost:4242";
var serverAddr = envAddr && envAddr.trim().length > 0 ? envAddr : typeof window !== "undefined" ? window.location.origin : "/";
var apiBase = serverAddr.endsWith("/") ? serverAddr.slice(0, -1) : serverAddr;
var peersCache = null;
var bearerToken = null;
var stateCache = null;
var getServerAddr = () => serverAddr;
var authHeaders = () => {
  if (!bearerToken)
    return;
  return { Authorization: `Bearer ${bearerToken}` };
};
var apiGet = async (path) => {
  const headers = authHeaders();
  const res = await fetch(`${apiBase}${path}`, {
    credentials: "include",
    headers
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
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
  const existing = peers.find((p) => p.id === peerId);
  if (existing) {
    return existing;
  }
  try {
    const localPeerId = await fetchLocalPeerId();
    if (localPeerId === peerId) {
      return { id: localPeerId, name: "Local node (you)" };
    }
  } catch {}
  return;
};
var fetchMimeTypes = async () => {
  const data = await apiGet("/api/mime-types");
  return data.mime_types;
};
var fetchUsers = async () => {
  const data = await apiGet("/users");
  return data.users ?? [];
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
  const headers = authHeaders();
  const res = await fetch(`${apiBase}/api/search?${params.toString()}`, {
    credentials: "include",
    headers
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
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
var listPeerDir = async (peerId, path) => {
  const params = new URLSearchParams;
  params.set("path", path.trim().length > 0 ? path : "/");
  const data = await apiGet(`/api/peers/${encodeURIComponent(peerId)}/dir?${params.toString()}`);
  return data.entries ?? [];
};
var listPeerDisks = async (peerId) => {
  const data = await apiGet(`/api/peers/${encodeURIComponent(peerId)}/disks`);
  return data.disks ?? [];
};
var fetchStorageUsage = async () => {
  const data = await apiGet("/api/storage");
  return data.files ?? [];
};
var fetchPeerCpus = async (peerId) => {
  const data = await apiGet(`/api/peers/${encodeURIComponent(peerId)}/cpus`);
  return data.cpus ?? [];
};
var fetchPeerInterfaces = async (peerId) => {
  const data = await apiGet(`/api/peers/${encodeURIComponent(peerId)}/interfaces`);
  return data.interfaces ?? [];
};
var fetchState = async () => {
  if (!stateCache) {
    stateCache = await apiGet("/api/state");
  }
  return stateCache;
};
var fetchLocalPeerId = async () => {
  const state = await fetchState();
  return state.me;
};
var fetchReleases = async (limit = 5) => {
  const res = await fetch(`https://api.github.com/repos/j45k4/puppynet/releases?per_page=${limit}`);
  if (!res.ok) {
    throw new Error(`GitHub releases request failed: ${res.status}`);
  }
  return await res.json();
};
var login = async (username, password, setCookie) => {
  const res = await fetch(`${apiBase}/auth/login`, {
    method: "POST",
    credentials: "include",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify({
      username,
      password,
      set_cookie: setCookie
    })
  });
  if (res.status === 401) {
    const message = await extractErrorMessage(res);
    throw new Error(message ?? "invalid credentials");
  }
  if (!res.ok) {
    const message = await extractErrorMessage(res);
    throw new Error(message ?? `Login failed: ${res.status}`);
  }
  return res.json();
};
async function extractErrorMessage(res) {
  try {
    const data = await res.json();
    if (data && typeof data.error === "string" && data.error.length > 0) {
      return data.error;
    }
  } catch {}
  return res.statusText;
}
var fetchMe = async () => {
  const headers = authHeaders();
  const res = await fetch(`${apiBase}/auth/me`, {
    credentials: "include",
    headers
  });
  if (res.status === 401) {
    return null;
  }
  if (!res.ok) {
    throw new Error(`Failed to load session: ${res.status}`);
  }
  const data = await res.json();
  return data.user ?? null;
};

// src/session.ts
var sessionChecked = false;
var sessionAuth = false;
var markSessionStatus = (authenticated) => {
  sessionChecked = true;
  sessionAuth = authenticated;
};
var isSessionAuthenticated = () => sessionAuth;
var hasSessionBeenChecked = () => sessionChecked;

// src/router.ts
var matcher;
async function ensureSession(path) {
  if (path === "/login")
    return true;
  if (isSessionAuthenticated())
    return true;
  if (hasSessionBeenChecked()) {
    return false;
  }
  try {
    const me = await fetchMe();
    markSessionStatus(!!me);
    return !!me;
  } catch {
    markSessionStatus(false);
    return false;
  }
}
var handleRoute = async (path) => {
  if (!matcher)
    return;
  const match = matcher.match(path);
  if (!match) {
    console.error("No route found for", path);
    return;
  }
  const requiresAuth = path !== "/login";
  if (requiresAuth && !await ensureSession(path)) {
    navigate("/login");
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

// src/layout.ts
var createRoot = () => {
  let root = document.getElementById("app-root");
  if (!root) {
    document.body.innerHTML = "";
    root = document.createElement("div");
    root.id = "app-root";
    document.body.appendChild(root);
  }
  return root;
};
var ensureShell = (currentPath) => {
  const root = createRoot();
  root.innerHTML = `
<div class="page shell">
		<nav class="nav">
			<a href="/" data-route="/">Home</a>
			<a href="/peers" data-route="/peers">Peers</a>
			<a href="/user" data-route="/user">Users</a>
			<a href="/files" data-route="/files">Files</a>
		<a href="/search" data-route="/search">Search</a>
		<a href="/storage" data-route="/storage">Storage</a>
		<a href="/updates" data-route="/updates">Updates</a>
		<a href="/settings" data-route="/settings">Settings</a>
	</nav>
	<main id="content" class="shell-content"></main>
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
var ensureLoginShell = () => {
  const root = createRoot();
  root.innerHTML = `
<div class="page login-page">
	<main id="content"></main>
</div>
`;
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
var escapeHtml = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
var formatFrequency = (hz) => {
  if (hz >= 1e9) {
    return `${(hz / 1e9).toFixed(2)} GHz`;
  }
  if (hz >= 1e6) {
    return `${(hz / 1e6).toFixed(2)} MHz`;
  }
  return `${hz} Hz`;
};
var formatBytes = (value) => {
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    index += 1;
    size /= 1024;
  }
  return `${size.toFixed(1)} ${units[index]}`;
};
var joinIps = (ips) => {
  if (!ips.length) {
    return "No IPs";
  }
  return ips.join(", ");
};
var renderCpuRows = (cpus) => cpus.map((cpu) => `
				<div class="resource-row">
					<div class="resource-name">
						<strong>${escapeHtml(cpu.name)}</strong>
					</div>
					<div class="resource-meta">
						<span>${cpu.usage.toFixed(1)}% usage</span>
						<span>${formatFrequency(cpu.frequency_hz)}</span>
					</div>
				</div>
			`).join("");
var renderInterfaceRows = (interfaces) => interfaces.map((iface) => `
				<div class="resource-row interface-row">
					<div class="resource-name">
						<strong>${escapeHtml(iface.name)}</strong>
						<p class="muted">
							${escapeHtml(iface.mac)}
							<span>${escapeHtml(joinIps(iface.ips))}</span>
						</p>
					</div>
					<div class="resource-meta">
						<span>Rx ${formatBytes(iface.total_received)}</span>
						<span>Tx ${formatBytes(iface.total_transmitted)}</span>
						<span>Pkts ${iface.packets_received}/${iface.packets_transmitted}</span>
						<span>Errors ${iface.errors_on_received}/${iface.errors_on_transmitted}</span>
					</div>
				</div>
			`).join("");
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
  let localPeerId = null;
  try {
    localPeerId = await fetchLocalPeerId();
  } catch {
    localPeerId = null;
  }
  let peers = [];
  let peerError = null;
  try {
    peers = await fetchPeers();
  } catch (error) {
    peerError = error instanceof Error ? error.message : String(error);
    peers = [];
  }
  const combined = [];
  if (localPeerId) {
    combined.push({
      id: localPeerId,
      name: "Local node (you)"
    });
  }
  for (const peer of peers) {
    if (localPeerId && peer.id === localPeerId) {
      continue;
    }
    combined.push(peer);
  }
  if (!tableEl)
    return;
  if (!combined.length) {
    const message = peerError ?? "No peers connected.";
    tableEl.innerHTML = `<p class="muted">${message}</p>`;
    if (statusEl)
      statusEl.textContent = message;
    return;
  }
  if (statusEl) {
    const baseMessage = `Showing ${combined.length} peer(s)`;
    statusEl.textContent = peerError ? `${baseMessage}; remote peers failed: ${peerError}` : baseMessage;
  }
  const rows = combined.map((peer) => `
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
		<div class="card" id="peer-cpu-card">
			<h2>CPUs</h2>
			<p id="peer-cpu-status" class="muted">CPU metrics will appear here.</p>
			<div id="peer-cpu-list" class="resource-list"></div>
		</div>
		<div class="card" id="peer-interfaces-card">
			<h2>Interfaces</h2>
			<p id="peer-interfaces-status" class="muted">Interface metrics will appear here.</p>
			<div id="peer-interfaces-list" class="resource-list"></div>
		</div>
	`;
  const backBtn = document.getElementById("back-to-peers");
  if (backBtn) {
    backBtn.addEventListener("click", () => navigate("/peers"));
  }
  const cpuStatusEl = document.getElementById("peer-cpu-status");
  const cpuListEl = document.getElementById("peer-cpu-list");
  const interfacesStatusEl = document.getElementById("peer-interfaces-status");
  const interfacesListEl = document.getElementById("peer-interfaces-list");
  const loadCpus = async () => {
    if (!cpuStatusEl || !cpuListEl)
      return;
    cpuStatusEl.textContent = "Loading CPU metrics...";
    cpuListEl.innerHTML = "";
    try {
      const cpus = await fetchPeerCpus(peerId);
      if (!cpus.length) {
        cpuListEl.innerHTML = `<p class="muted">No CPU data reported.</p>`;
        cpuStatusEl.textContent = "No CPU data available.";
        return;
      }
      cpuListEl.innerHTML = renderCpuRows(cpus);
      cpuStatusEl.textContent = `Loaded ${cpus.length} CPU core(s)`;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      cpuStatusEl.textContent = `Failed to load CPU info: ${message}`;
      cpuListEl.innerHTML = `<p class="muted">Failed to load CPU data.</p>`;
    }
  };
  const loadInterfaces = async () => {
    if (!interfacesStatusEl || !interfacesListEl)
      return;
    interfacesStatusEl.textContent = "Loading interface metrics...";
    interfacesListEl.innerHTML = "";
    try {
      const interfaces = await fetchPeerInterfaces(peerId);
      if (!interfaces.length) {
        interfacesListEl.innerHTML = `<p class="muted">No interfaces reported.</p>`;
        interfacesStatusEl.textContent = "No interface data available.";
        return;
      }
      interfacesListEl.innerHTML = renderInterfaceRows(interfaces);
      interfacesStatusEl.textContent = `Loaded ${interfaces.length} interface(s)`;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      interfacesStatusEl.textContent = `Failed to load interfaces: ${message}`;
      interfacesListEl.innerHTML = `<p class="muted">Failed to load interface data.</p>`;
    }
  };
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
    loadCpus();
    loadInterfaces();
  } catch (err) {
    const statusEl = document.getElementById("peer-status");
    if (statusEl)
      statusEl.textContent = `Failed to load peer: ${err}`;
  }
};

// src/pages/files.ts
var formatSize = (value) => {
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    index += 1;
    size /= 1024;
  }
  return `${size.toFixed(1)} ${units[index]}`;
};
var escapeHtml2 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
var joinChildPath = (base, child) => {
  const trimmedChild = child.replace(/^[\\/]+/, "").replace(/[\\/]+$/, "");
  if (!trimmedChild) {
    return base || child;
  }
  if (!base) {
    return trimmedChild;
  }
  const endsWithSeparator = base.endsWith("/") || base.endsWith("\\");
  if (endsWithSeparator) {
    return `${base}${trimmedChild}`;
  }
  const separator = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  return `${base}${separator}${trimmedChild}`;
};
var parentPath = (path) => {
  const trimmed = path.trim();
  if (!trimmed || trimmed === "/") {
    return null;
  }
  const driveRoot = trimmed.match(/^[A-Za-z]:\\?$/);
  if (driveRoot) {
    return "";
  }
  const withoutTrailing = trimmed.replace(/[\\/]+$/, "");
  if (!withoutTrailing) {
    return null;
  }
  const lastSep = Math.max(withoutTrailing.lastIndexOf("/"), withoutTrailing.lastIndexOf("\\"));
  if (lastSep === -1) {
    return "";
  }
  return withoutTrailing.slice(0, lastSep + 1);
};
var renderFiles = async () => {
  const content = ensureShell("/files");
  content.innerHTML = `
	<section class="hero">
		<h1>Files</h1>
		<p class="lede">Browse shared directories and remote disks.</p>
	</section>
	<div class="card" id="files-card">
		<div class="card-heading">
			<h2>File browser</h2>
			<p id="files-status" class="muted">Loading peers...</p>
		</div>
		<div class="files-controls">
			<label for="files-peer-select">Peer</label>
			<select id="files-peer-select"></select>
			<button type="button" id="files-refresh">Refresh view</button>
		</div>
		<div class="files-toolbar">
			<button type="button" id="files-up">Up</button>
			<div class="files-path-label">
				<span class="muted">Path:</span>
				<strong id="files-path-value">Disks</strong>
			</div>
			<div class="files-path-entry">
				<input id="files-path-input" placeholder="/path/to/folder" />
				<button type="button" id="files-go">Browse</button>
			</div>
		</div>
		<div id="files-browser" class="files-browser"></div>
	</div>
`;
  const statusEl = content.querySelector("#files-status");
  const peerSelect = content.querySelector("#files-peer-select");
  const browserEl = content.querySelector("#files-browser");
  const upButton = content.querySelector("#files-up");
  const refreshButton = content.querySelector("#files-refresh");
  const goButton = content.querySelector("#files-go");
  const pathInput = content.querySelector("#files-path-input");
  const pathValueEl = content.querySelector("#files-path-value");
  const state = {
    peerId: "",
    path: "",
    showingDisks: true,
    entries: [],
    disks: [],
    loading: false,
    error: null
  };
  const updateControls = () => {
    const disabled = state.loading;
    if (peerSelect) {
      peerSelect.disabled = disabled;
    }
    if (refreshButton) {
      refreshButton.disabled = disabled || !state.peerId;
    }
    if (upButton) {
      upButton.disabled = disabled || state.showingDisks || !state.path.trim().length;
    }
    if (pathInput) {
      pathInput.disabled = disabled || !state.peerId;
    }
    if (goButton) {
      goButton.disabled = disabled || !state.peerId;
    }
  };
  const renderBrowser = () => {
    if (!browserEl)
      return;
    if (state.loading) {
      browserEl.innerHTML = `<p class="muted">Loading ${state.showingDisks ? "disks" : "directory"}...</p>`;
      return;
    }
    if (state.error) {
      browserEl.innerHTML = `<p class="muted">Error: ${escapeHtml2(state.error)}</p>`;
      return;
    }
    if (state.showingDisks) {
      if (!state.disks.length) {
        browserEl.innerHTML = `<p class="muted">No disks were reported for this peer.</p>`;
        return;
      }
      const rows2 = state.disks.map((disk) => {
        const label = disk.name || disk.mount_path;
        return `
					<div class="files-row">
						<div>
							<strong>${escapeHtml2(label)}</strong>
							<p class="muted">${escapeHtml2(disk.mount_path)}</p>
							<p>${formatSize(disk.available_space)} free of ${formatSize(disk.total_space)}</p>
						</div>
						<button type="button" class="link-btn" data-disk-path="${escapeHtml2(disk.mount_path)}">Browse</button>
					</div>
				`;
      }).join("");
      browserEl.innerHTML = `<div class="files-list">${rows2}</div>`;
      const diskButtons = browserEl.querySelectorAll("[data-disk-path]");
      diskButtons.forEach((btn) => {
        btn.addEventListener("click", () => {
          const diskPath = btn.dataset.diskPath;
          if (!diskPath)
            return;
          state.showingDisks = false;
          state.path = diskPath;
          state.entries = [];
          state.error = null;
          loadBrowser();
        });
      });
      return;
    }
    if (!state.entries.length) {
      browserEl.innerHTML = `<p class="muted">Directory is empty.</p>`;
      return;
    }
    const rows = state.entries.map((entry) => {
      const label = entry.is_dir ? `[DIR] ${escapeHtml2(entry.name)}` : escapeHtml2(entry.name);
      const meta = entry.is_dir ? "Directory" : `${entry.mime ?? "File"} • ${formatSize(entry.size)}`;
      return `
				<button
					type="button"
					class="files-entry"
					data-entry-name="${escapeHtml2(entry.name)}"
					data-entry-dir="${entry.is_dir ? "1" : "0"}"
				>
					<div>
						<strong>${label}</strong>
						<p class="muted">${meta}</p>
					</div>
					<span class="badge small">${entry.is_dir ? "dir" : "file"}</span>
				</button>
			`;
    }).join("");
    browserEl.innerHTML = `<div class="files-list">${rows}</div>`;
    const entryButtons = browserEl.querySelectorAll("[data-entry-name]");
    entryButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const name = btn.dataset.entryName;
        if (!name)
          return;
        const isDir = btn.dataset.entryDir === "1";
        const target = joinChildPath(state.path, name);
        if (isDir) {
          state.showingDisks = false;
          state.path = target;
          state.entries = [];
          state.error = null;
          loadBrowser();
          return;
        }
        if (statusEl) {
          statusEl.textContent = `Selected ${target}`;
        }
      });
    });
  };
  const updateBrowserView = () => {
    updateControls();
    if (pathValueEl) {
      pathValueEl.textContent = state.showingDisks ? "Disks" : state.path || "/";
    }
    if (statusEl) {
      if (state.error) {
        statusEl.textContent = `Error: ${state.error}`;
      } else if (state.loading) {
        statusEl.textContent = `Loading ${state.showingDisks ? "disks" : "directory"}...`;
      } else if (state.showingDisks) {
        statusEl.textContent = "Select a disk to browse.";
      } else {
        statusEl.textContent = `Browsing ${state.path || "/"}`;
      }
    }
    renderBrowser();
  };
  const loadBrowser = async () => {
    if (!state.peerId) {
      state.error = "Select a peer first.";
      updateBrowserView();
      return;
    }
    state.loading = true;
    state.error = null;
    if (state.showingDisks) {
      state.disks = [];
    } else {
      state.entries = [];
    }
    updateBrowserView();
    try {
      if (state.showingDisks) {
        state.disks = await listPeerDisks(state.peerId);
      } else {
        const targetPath = state.path.trim().length ? state.path : "/";
        state.entries = await listPeerDir(state.peerId, targetPath);
      }
    } catch (err) {
      state.error = err instanceof Error ? err.message : String(err);
    } finally {
      state.loading = false;
      updateBrowserView();
    }
  };
  const selectPeer = (peerId) => {
    if (!peerId || state.peerId === peerId) {
      return;
    }
    state.peerId = peerId;
    state.path = "";
    state.entries = [];
    state.disks = [];
    state.showingDisks = true;
    state.error = null;
    loadBrowser();
  };
  const handleGo = () => {
    const target = pathInput?.value.trim() ?? "";
    if (!state.peerId) {
      return;
    }
    state.showingDisks = false;
    state.path = target || "/";
    state.entries = [];
    state.error = null;
    loadBrowser();
  };
  const handleUp = () => {
    if (state.showingDisks || !state.path) {
      return;
    }
    const next = parentPath(state.path);
    if (next === null) {
      return;
    }
    if (next === "") {
      state.showingDisks = true;
      state.path = "";
      state.entries = [];
      state.error = null;
      loadBrowser();
      return;
    }
    state.showingDisks = false;
    state.path = next;
    state.entries = [];
    state.error = null;
    loadBrowser();
  };
  const formatPeerOption = (peerId, label) => `<option value="${escapeHtml2(peerId)}">${escapeHtml2(label)}</option>`;
  const describeError = (error) => error instanceof Error ? error.message : String(error);
  const loadPeers = async () => {
    if (peerSelect) {
      peerSelect.disabled = true;
      peerSelect.innerHTML = `<option value="">Loading peers…</option>`;
    }
    let localPeerId = null;
    try {
      localPeerId = await fetchLocalPeerId();
    } catch {
      localPeerId = null;
    }
    let peerError = null;
    let peers = [];
    try {
      peers = await fetchPeers();
    } catch (err) {
      peerError = describeError(err);
      peers = [];
    }
    if (!peerSelect) {
      return;
    }
    const options = [];
    if (localPeerId) {
      options.push({
        id: localPeerId,
        label: `Local node (you) (${localPeerId})`
      });
    }
    for (const peer of peers) {
      if (peer.id === localPeerId) {
        continue;
      }
      options.push({
        id: peer.id,
        label: `${peer.name ?? "Unnamed"} (${peer.id})`
      });
    }
    if (!options.length) {
      const message = peerError ?? "No peers connected";
      peerSelect.innerHTML = `<option value="">${escapeHtml2(message)}</option>`;
      if (statusEl)
        statusEl.textContent = message;
      peerSelect.disabled = false;
      return;
    }
    peerSelect.innerHTML = options.map(({ id, label }) => formatPeerOption(id, label)).join("");
    const firstPeer = options[0];
    peerSelect.value = firstPeer.id;
    selectPeer(firstPeer.id);
    if (statusEl) {
      if (peerError) {
        statusEl.textContent = `Loaded ${options.length} peer(s); remote peers failed: ${peerError}`;
      } else {
        statusEl.textContent = `${options.length} peer(s)`;
      }
    }
    if (peerSelect) {
      peerSelect.disabled = false;
    }
  };
  peerSelect?.addEventListener("change", () => {
    const peerId = peerSelect.value;
    selectPeer(peerId);
  });
  refreshButton?.addEventListener("click", () => {
    loadBrowser();
  });
  goButton?.addEventListener("click", () => {
    handleGo();
  });
  upButton?.addEventListener("click", () => {
    handleUp();
  });
  pathInput?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      handleGo();
    }
  });
  updateBrowserView();
  loadPeers();
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
var formatSize2 = (value) => {
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    index += 1;
    size /= 1024;
  }
  return `${size.toFixed(1)} ${units[index]}`;
};
var escapeHtml3 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
var formatTimestamp = (value) => {
  if (!value) {
    return "-";
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return parsed.toLocaleString();
};
var normalizePath = (value) => value.replace(/\\/g, "/").replace(/^\/+/, "").replace(/\/+$/, "");
var latestTimestamp = (current, candidate) => {
  if (!current) {
    return candidate;
  }
  if (!candidate) {
    return current;
  }
  return current >= candidate ? current : candidate;
};
var formatNodeId = (bytes) => {
  if (!bytes.length) {
    return "unknown";
  }
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
};
var displayName = (path) => {
  if (!path) {
    return "Root";
  }
  const segments = path.split("/").filter((segment) => segment.length > 0);
  if (!segments.length) {
    return "Root";
  }
  return segments[segments.length - 1];
};
var buildStorageNodes = (files) => {
  if (!files.length) {
    return [];
  }
  const grouped = new Map;
  for (const file of files) {
    const key = file.node_id.join(",");
    const existing = grouped.get(key);
    if (existing) {
      existing.records.push(file);
    } else {
      grouped.set(key, {
        name: file.node_name || formatNodeId(file.node_id),
        id: file.node_id,
        records: [file]
      });
    }
  }
  const nodes = [];
  grouped.forEach(({ name, id, records }) => {
    const { entries, totalSize } = buildStorageTree(records);
    nodes.push({
      name: name || formatNodeId(id),
      id: formatNodeId(id),
      totalSize,
      entries
    });
  });
  nodes.sort((a, b) => a.name.localeCompare(b.name));
  return nodes;
};
var buildStorageTree = (files) => {
  const stats = new Map;
  const children = new Map;
  for (const file of files) {
    const normalized = normalizePath(file.path);
    const ancestors = [""];
    if (normalized.length) {
      let current = "";
      for (const segment of normalized.split("/")) {
        if (!segment) {
          continue;
        }
        current = current ? `${current}/${segment}` : segment;
        ancestors.push(current);
      }
    }
    for (const path of ancestors) {
      const existing = stats.get(path);
      const updated = {
        size: (existing?.size ?? 0) + file.size,
        itemCount: (existing?.itemCount ?? 0) + 1,
        lastChanged: latestTimestamp(existing?.lastChanged ?? null, file.last_changed ?? null)
      };
      stats.set(path, updated);
    }
    for (let i = 0;i < ancestors.length - 1; i += 1) {
      const parent = ancestors[i];
      const child = ancestors[i + 1];
      if (parent === undefined || child === undefined) {
        continue;
      }
      const set = children.get(parent) ?? new Set;
      set.add(child);
      children.set(parent, set);
    }
  }
  const rootStats = stats.get("");
  const totalSize = rootStats?.size ?? 0;
  const entries = buildStorageEntriesFor("", stats, children, totalSize);
  return { entries, totalSize };
};
var buildStorageEntriesFor = (parent, stats, children, totalSize) => {
  const childPaths = children.get(parent);
  if (!childPaths) {
    return [];
  }
  const sorted = Array.from(childPaths).sort((a, b) => {
    const aSize = stats.get(a)?.size ?? 0;
    const bSize = stats.get(b)?.size ?? 0;
    return bSize - aSize;
  });
  return sorted.map((childPath) => {
    const data = stats.get(childPath);
    if (!data) {
      return null;
    }
    const percent = totalSize === 0 ? 0 : data.size / totalSize * 100;
    return {
      path: childPath,
      name: displayName(childPath),
      size: data.size,
      itemCount: data.itemCount,
      lastChanged: data.lastChanged,
      percent,
      children: buildStorageEntriesFor(childPath, stats, children, data.size)
    };
  }).filter((entry) => entry !== null);
};
var renderStorage = async () => {
  const content = ensureShell("/storage");
  content.innerHTML = `
	<section class="hero">
		<h1>Storage</h1>
		<p class="lede">Storage usage summary for shared folders and nodes.</p>
	</section>
	<div class="card" id="storage-card">
		<div class="card-heading">
			<h2>Storage usage overview</h2>
			<p id="storage-status" class="muted">Loading storage usage...</p>
		</div>
		<div class="storage-actions">
			<button type="button" id="storage-refresh">Refresh</button>
		</div>
		<div class="storage-table">
			<div class="storage-row storage-row-header">
				<div class="storage-cell storage-name">Name</div>
				<div class="storage-cell">% of node</div>
				<div class="storage-cell">Size</div>
				<div class="storage-cell">Items</div>
				<div class="storage-cell">Last changed</div>
				<div class="storage-cell">Action</div>
			</div>
		</div>
		<div id="storage-list" class="storage-list"></div>
	</div>
`;
  const statusEl = content.querySelector("#storage-status");
  const listEl = content.querySelector("#storage-list");
  const refreshButton = content.querySelector("#storage-refresh");
  const state = {
    nodes: [],
    loading: true,
    error: null,
    expandedNodes: new Set,
    expandedEntries: new Set,
    customStatus: null
  };
  const updateStatus = () => {
    if (!statusEl)
      return;
    if (state.customStatus) {
      statusEl.textContent = state.customStatus;
      return;
    }
    if (state.loading) {
      statusEl.textContent = "Loading storage usage...";
    } else if (state.error) {
      statusEl.textContent = `Failed to load storage usage: ${state.error}`;
    } else if (!state.nodes.length) {
      statusEl.textContent = "No storage data available.";
    } else {
      statusEl.textContent = `Showing ${state.nodes.length} node(s)`;
    }
  };
  const renderEntries = (nodeIndex, entries, depth) => entries.map((entry) => {
    const entryKey = `${nodeIndex}:${entry.path}`;
    const isExpanded = state.expandedEntries.has(entryKey);
    const hasChildren = entry.children.length > 0;
    const toggleLabel = hasChildren ? isExpanded ? "▾" : "▸" : "";
    const openButton = hasChildren ? "" : `<button type="button" class="link-btn" data-entry-open="${escapeHtml3(entry.path)}">Open</button>`;
    return `
					<div class="storage-row storage-entry-row">
						<div class="storage-cell storage-entry-name">
							${hasChildren ? `<button type="button" class="link-btn" data-entry-toggle="${entryKey}">${toggleLabel}</button>` : '<span class="storage-toggle-placeholder"></span>'}
							<div class="storage-name-content" style="margin-left: ${depth * 16}px">
								<strong>${escapeHtml3(entry.name)}</strong>
								<p class="muted">${escapeHtml3(entry.path)}</p>
							</div>
						</div>
						<div class="storage-cell">${entry.percent.toFixed(1)}%</div>
						<div class="storage-cell">${formatSize2(entry.size)}</div>
						<div class="storage-cell">${entry.itemCount}</div>
						<div class="storage-cell">${formatTimestamp(entry.lastChanged)}</div>
						<div class="storage-cell">${openButton}</div>
					</div>
					${isExpanded ? `<div class="storage-entry-children">${renderEntries(nodeIndex, entry.children, depth + 1)}</div>` : ""}
				`;
  }).join("");
  const renderNode = (node, index) => {
    const isExpanded = state.expandedNodes.has(index);
    const toggleLabel = node.entries.length ? isExpanded ? "▾" : "▸" : "";
    return `
			<div class="storage-node">
				<div class="storage-row storage-node-row">
					<div class="storage-cell storage-name">
						${node.entries.length ? `<button type="button" class="link-btn" data-node-index="${index}">${toggleLabel}</button>` : '<span class="storage-toggle-placeholder"></span>'}
						<div class="storage-name-content">
							<strong>${escapeHtml3(node.name)}</strong>
							<p class="muted storage-node-id">${escapeHtml3(node.id)}</p>
						</div>
					</div>
					<div class="storage-cell">100%</div>
					<div class="storage-cell">${formatSize2(node.totalSize)}</div>
					<div class="storage-cell">-</div>
					<div class="storage-cell muted">-</div>
					<div class="storage-cell"></div>
				</div>
				${isExpanded ? `<div class="storage-entries">${renderEntries(index, node.entries, 1)}</div>` : ""}
			</div>
		`;
  };
  const updateStorageView = () => {
    if (!listEl)
      return;
    if (state.loading) {
      listEl.innerHTML = `<p class="muted">Loading storage usage...</p>`;
    } else if (state.error) {
      const errorMessage = escapeHtml3(state.error ?? "Unknown error");
      listEl.innerHTML = `<p class="muted">Error: ${errorMessage}</p>`;
    } else if (!state.nodes.length) {
      listEl.innerHTML = `<p class="muted">No storage data available.</p>`;
    } else {
      listEl.innerHTML = state.nodes.map(renderNode).join("");
    }
    updateStatus();
    if (!listEl)
      return;
    const nodeButtons = listEl.querySelectorAll("[data-node-index]");
    nodeButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const value = btn.getAttribute("data-node-index");
        if (!value)
          return;
        const index = Number(value);
        if (state.expandedNodes.has(index)) {
          state.expandedNodes.delete(index);
        } else {
          state.expandedNodes.add(index);
        }
        updateStorageView();
      });
    });
    const entryToggleButtons = listEl.querySelectorAll("[data-entry-toggle]");
    entryToggleButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const key = btn.getAttribute("data-entry-toggle");
        if (!key)
          return;
        if (state.expandedEntries.has(key)) {
          state.expandedEntries.delete(key);
        } else {
          state.expandedEntries.add(key);
        }
        updateStorageView();
      });
    });
    const entryOpenButtons = listEl.querySelectorAll("[data-entry-open]");
    entryOpenButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const path = btn.getAttribute("data-entry-open");
        if (!path)
          return;
        state.customStatus = `Selected ${path}`;
        updateStatus();
      });
    });
  };
  const loadStorage = async () => {
    state.loading = true;
    state.error = null;
    state.customStatus = null;
    state.nodes = [];
    state.expandedNodes.clear();
    state.expandedEntries.clear();
    updateStorageView();
    try {
      const files = await fetchStorageUsage();
      state.nodes = buildStorageNodes(files);
    } catch (error) {
      state.error = error instanceof Error ? error.message : String(error);
    } finally {
      state.loading = false;
      updateStorageView();
    }
  };
  refreshButton?.addEventListener("click", () => {
    loadStorage();
  });
  updateStorageView();
  loadStorage();
};

// src/pages/updates.ts
var formatDate = (value) => {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
};
var escapeHtml4 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
var snippet = (value, limit = 280) => {
  if (!value) {
    return "";
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return "";
  }
  const shortened = trimmed.split(`
`).map((line) => line.trim()).join(" ");
  return shortened.length <= limit ? shortened : `${shortened.slice(0, limit)}…`;
};
var formatBytes2 = (value) => {
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    index += 1;
    size /= 1024;
  }
  return `${size.toFixed(1)} ${units[index]}`;
};
var renderAssets = (assets) => {
  if (!assets.length) {
    return `<p class="muted">No downloadable assets.</p>`;
  }
  return `
		<ul class="updates-assets">
			${assets.map((asset) => `
						<li>
							<a href="${asset.browser_download_url}" target="_blank" rel="noreferrer">
								${asset.name}
							</a>
							<span>${formatBytes2(asset.size)}</span>
						</li>
					`).join("")}
		</ul>
	`;
};
var renderReleaseNotes = (body) => {
  if (!body || !body.trim().length) {
    return `<p class="muted">No release notes provided.</p>`;
  }
  return `
		<div class="release-notes">
			<pre>${escapeHtml4(body)}</pre>
		</div>
	`;
};
var renderRelease = (release) => {
  const title = release.name || release.tag_name;
  const badge = release.prerelease ? '<span class="badge">Pre-release</span>' : "";
  const summary = snippet(release.body);
  return `
		<div class="updates-card">
			<div class="updates-card__header">
				<div>
					<h3>${title}</h3>
					<p class="muted">${release.tag_name} • ${formatDate(release.published_at)}</p>
				</div>
				<div class="updates-card__badge">
					${badge}
					<a href="${release.html_url}" target="_blank" rel="noreferrer" class="link-btn">View on GitHub</a>
				</div>
			</div>
			${summary ? `<p>${summary}</p>` : ""}
			${renderReleaseNotes(release.body)}
			${renderAssets(release.assets)}
		</div>
	`;
};
var renderUpdates = async () => {
  const content = ensureShell("/updates");
  content.innerHTML = `
	<section class="hero">
		<h1>Updates</h1>
		<p class="lede">Latest published versions of PuppyNet from GitHub releases.</p>
	</section>
	<div class="card" id="updates-card">
		<div class="card-heading">
			<h2>Release feed</h2>
			<p id="updates-status" class="muted">Fetching releases…</p>
			<button type="button" id="updates-refresh">Refresh</button>
		</div>
		<div id="updates-list" class="updates-list"></div>
	</div>
`;
  const statusEl = document.getElementById("updates-status");
  const listEl = document.getElementById("updates-list");
  const refreshButton = document.getElementById("updates-refresh");
  const loadReleases = async () => {
    if (statusEl)
      statusEl.textContent = "Loading latest releases…";
    if (listEl)
      listEl.innerHTML = "";
    try {
      const releases = await fetchReleases(5);
      if (!listEl)
        return;
      if (!releases.length) {
        listEl.innerHTML = `<p class="muted">No releases were found.</p>`;
        if (statusEl)
          statusEl.textContent = "No releases available.";
        return;
      }
      listEl.innerHTML = releases.map((release) => renderRelease(release)).join("");
      if (statusEl)
        statusEl.textContent = `Showing ${releases.length} release(s)`;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (statusEl)
        statusEl.textContent = `Failed to load releases: ${message}`;
      if (listEl)
        listEl.innerHTML = `<p class="muted">Unable to reach GitHub releases.</p>`;
    }
  };
  refreshButton?.addEventListener("click", () => {
    loadReleases();
  });
  loadReleases();
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

// src/pages/users.ts
var renderUsers = async () => {
  const content = ensureShell("/user");
  content.innerHTML = `
		<section class="hero">
			<h1>Users</h1>
			<p class="lede">PuppyNet remembers every user that can sign into the network.</p>
		</section>
		<div class="card">
			<h2>Known accounts</h2>
			<p id="users-status" class="muted">Loading users...</p>
			<div id="users-list"></div>
		</div>
	`;
  const statusEl = document.getElementById("users-status");
  const listEl = document.getElementById("users-list");
  try {
    const users = await fetchUsers();
    if (statusEl)
      statusEl.textContent = `${users.length} user(s)`;
    if (!listEl)
      return;
    if (users.length === 0) {
      listEl.innerHTML = `<p class="muted">No users registered yet.</p>`;
      return;
    }
    const rows = users.map((name) => `<li><button class="link-btn" data-user="${name}">${name}</button></li>`).join("");
    listEl.innerHTML = `<ul class="users-list">${rows}</ul>`;
    const buttons = listEl.querySelectorAll("[data-user]");
    buttons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const username = btn.getAttribute("data-user");
        if (username)
          navigate(`/user/${encodeURIComponent(username)}`);
      });
    });
  } catch (err) {
    if (statusEl)
      statusEl.textContent = `Failed to load users: ${err}`;
  }
};
var renderUserDetail = (userId) => {
  const content = ensureShell("/user");
  content.innerHTML = `
		<section class="hero">
			<h1>User</h1>
			<p class="lede">Profile for <strong>${userId}</strong></p>
		</section>
		<div class="card">
			<h2>Details</h2>
			<p class="muted">Username: ${userId}</p>
			<button class="link-btn" id="back-to-users">Back to users</button>
		</div>
	`;
  const back = document.getElementById("back-to-users");
  back?.addEventListener("click", () => navigate("/user"));
};

// src/pages/login.ts
var statusMsg = (el, msg) => {
  if (el)
    el.textContent = msg;
};
var renderLogin = async () => {
  const content = ensureLoginShell();
  content.innerHTML = `
		<section class="hero">
			<h1>Sign in to PuppyNet</h1>
			<p class="lede">Log in with your PuppyNet credentials to continue.</p>
		</section>
		<div class="card login-card">
			<form id="login-form">
				<input id="login-username" name="username" placeholder="Username" autocomplete="username" required />
				<input id="login-password" name="password" type="password" placeholder="Password" autocomplete="current-password" required />
				<button type="submit">Sign in</button>
			</form>
			<p id="login-status" class="login-status"></p>
		</div>
	`;
  const form = document.getElementById("login-form");
  const usernameInput = document.getElementById("login-username");
  const passwordInput = document.getElementById("login-password");
  const statusEl = document.getElementById("login-status");
  form?.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const username = usernameInput?.value.trim() ?? "";
    const password = passwordInput?.value ?? "";
    if (!username || !password) {
      statusMsg(statusEl, "Please enter username and password.");
      return;
    }
    statusMsg(statusEl, "Signing in...");
    try {
      await login(username, password, true);
      markSessionStatus(true);
      navigate("/");
    } catch (err) {
      if (err instanceof Error) {
        statusMsg(statusEl, err.message);
      } else {
        statusMsg(statusEl, `Login failed: ${String(err)}`);
      }
    }
  });
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
    "/login": () => renderLogin(),
    "/peers": () => renderPeers(),
    "/peers/:peerId": ({ peerId }) => renderPeerDetail(peerId),
    "/user": () => renderUsers(),
    "/user/:userId": ({ userId }) => renderUserDetail(userId),
    "/files": () => renderFiles(),
    "/search": () => renderSearch(),
    "/storage": () => renderStorage(),
    "/updates": () => renderUpdates(),
    "/settings": () => renderSettings(),
    "/*": () => renderHome()
  });
  console.info("Using server address:", serverAddr2);
};
