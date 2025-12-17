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
var parseUpdateProgress = (raw) => {
  const entries = Object.entries(raw);
  if (entries.length !== 1) {
    return null;
  }
  const [first] = entries;
  if (!first) {
    return null;
  }
  const [key, value] = first;
  switch (key) {
    case "FetchingRelease":
      return { type: "FetchingRelease" };
    case "Downloading": {
      if (value && typeof value === "object" && "filename" in value && typeof value.filename === "string") {
        return { type: "Downloading", filename: value.filename };
      }
      return null;
    }
    case "Unpacking":
      return { type: "Unpacking" };
    case "Verifying":
      return { type: "Verifying" };
    case "Installing":
      return { type: "Installing" };
    case "Completed": {
      if (value && typeof value === "object" && "version" in value && typeof value.version === "string") {
        return { type: "Completed", version: value.version };
      }
      return null;
    }
    case "Failed": {
      if (value && typeof value === "object" && "error" in value && typeof value.error === "string") {
        return { type: "Failed", error: value.error };
      }
      return null;
    }
    case "AlreadyUpToDate": {
      if (value && typeof value === "object" && "current_version" in value && typeof value.current_version === "number") {
        return { type: "AlreadyUpToDate", current_version: value.current_version };
      }
      return null;
    }
    default:
      return null;
  }
};
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
var fetchFileByHash = async (hash) => {
  const headers = authHeaders();
  const res = await fetch(`${apiBase}/api/file/hash?hash=${encodeURIComponent(hash)}`, {
    method: "GET",
    credentials: "include",
    headers
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
  if (!res.ok) {
    let errorMessage = `Request failed: ${res.status}`;
    try {
      const payload = await res.json();
      if (payload && typeof payload.error === "string") {
        errorMessage = payload.error;
      }
    } catch {}
    throw new Error(errorMessage);
  }
  const arrayBuffer = await res.arrayBuffer();
  const mime = res.headers.get("content-type") ?? "application/octet-stream";
  const lengthHeader = res.headers.get("content-length");
  return {
    data: new Uint8Array(arrayBuffer),
    mime,
    length: lengthHeader ? Number(lengthHeader) : arrayBuffer.byteLength,
    status: res.status
  };
};
var startPeerShell = async (peerId) => {
  const headers = authHeaders();
  const res = await fetch(`${apiBase}/api/peers/${encodeURIComponent(peerId)}/shell/start`, {
    method: "POST",
    credentials: "include",
    headers
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
  if (!res.ok) {
    throw new Error(`Failed to start shell: ${res.status}`);
  }
  const data = await res.json();
  return data.id;
};
var sendPeerShellInput = async (peerId, id, data) => {
  const auth = authHeaders();
  const headers = {
    "content-type": "application/json",
    ...auth ?? {}
  };
  const res = await fetch(`${apiBase}/api/peers/${encodeURIComponent(peerId)}/shell/input`, {
    method: "POST",
    credentials: "include",
    headers,
    body: JSON.stringify({ id, data })
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
  if (!res.ok) {
    throw new Error(`Shell input failed: ${res.status}`);
  }
  const payload = await res.json();
  return payload.data;
};
var fetchPeerFileChunk = async (peerId, path, length) => {
  const params = new URLSearchParams;
  params.set("path", path);
  if (length !== undefined) {
    params.set("length", String(length));
  }
  return apiGet(`/api/peers/${encodeURIComponent(peerId)}/file?${params.toString()}`);
};
var startPeerUpdate = async (peerId, version) => {
  const auth = authHeaders();
  const headers = {
    "content-type": "application/json",
    ...auth ?? {}
  };
  const res = await fetch(`${apiBase}/api/updates/${encodeURIComponent(peerId)}`, {
    method: "POST",
    credentials: "include",
    headers,
    body: JSON.stringify({ version })
  });
  if (res.status === 401) {
    throw new Error("not authenticated");
  }
  if (!res.ok) {
    throw new Error(`Failed to start update: ${res.status}`);
  }
  const data = await res.json();
  return data.update_id;
};
var pollPeerUpdate = async (updateId) => {
  const data = await apiGet(`/api/updates/${updateId}/events`);
  return data.events.map((event) => parseUpdateProgress(event)).filter((event) => event !== null);
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
var formatUpdateMessage = (event) => {
  switch (event.type) {
    case "FetchingRelease":
      return "Fetching release metadata";
    case "Downloading":
      return `Downloading ${event.filename}`;
    case "Unpacking":
      return "Unpacking update";
    case "Verifying":
      return "Verifying update";
    case "Installing":
      return "Installing update";
    case "Completed":
      return `Update completed (version ${event.version})`;
    case "Failed":
      return `Update failed: ${event.error}`;
    case "AlreadyUpToDate":
      return `Already up to date (version ${event.current_version})`;
  }
};
var describeError = (error) => error instanceof Error ? error.message : String(error);
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
			<div class="peer-actions" style="margin-top: 8px;">
				<button type="button" class="link-btn" id="peer-open-shell">Remote shell</button>
			</div>
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
		<div class="card" id="peer-update-card">
			<h2>Updates</h2>
			<p id="peer-update-status" class="muted">Update status will appear here.</p>
			<button type="button" id="peer-update-button">Update peer</button>
			<div id="peer-update-log" class="updates-list"></div>
		</div>
	`;
  const backBtn = document.getElementById("back-to-peers");
  if (backBtn) {
    backBtn.addEventListener("click", () => navigate("/peers"));
  }
  const shellBtn = document.getElementById("peer-open-shell");
  if (shellBtn) {
    shellBtn.addEventListener("click", () => navigate(`/peers/${peerId}/shell`));
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
  const updateStatusEl = document.getElementById("peer-update-status");
  const updateLogEl = document.getElementById("peer-update-log");
  const updateButton = document.getElementById("peer-update-button");
  const updateState = {
    updateId: null,
    inProgress: false,
    done: false,
    events: [],
    error: null
  };
  let updatePoll = null;
  const renderUpdateLog = () => {
    if (!updateLogEl)
      return;
    if (!updateState.events.length) {
      updateLogEl.innerHTML = `<p class="muted">No update activity yet.</p>`;
      return;
    }
    updateLogEl.innerHTML = updateState.events.map((event) => `
					<div class="update-event update-event-${event.type.toLowerCase()}">
						${escapeHtml(formatUpdateMessage(event))}
					</div>
				`).join("");
  };
  const setUpdateStatus = (message) => {
    if (updateStatusEl) {
      updateStatusEl.textContent = message;
    }
  };
  const setUpdateButtonState = () => {
    if (updateButton) {
      updateButton.disabled = updateState.inProgress;
    }
  };
  const clearUpdatePoll = () => {
    if (updatePoll !== null) {
      window.clearTimeout(updatePoll);
      updatePoll = null;
    }
  };
  const pollUpdates = async () => {
    if (!updateState.updateId) {
      return;
    }
    try {
      const events = await pollPeerUpdate(updateState.updateId);
      if (events.length > 0) {
        updateState.events.push(...events);
        renderUpdateLog();
        const last = events[events.length - 1];
        setUpdateStatus(formatUpdateMessage(last));
        if (last.type === "Completed" || last.type === "Failed" || last.type === "AlreadyUpToDate") {
          updateState.inProgress = false;
          updateState.done = true;
          clearUpdatePoll();
          setUpdateButtonState();
          return;
        }
      }
    } catch (error) {
      updateState.error = describeError(error);
      updateState.inProgress = false;
      setUpdateStatus(`Update polling failed: ${updateState.error}`);
      setUpdateButtonState();
      clearUpdatePoll();
      return;
    }
    if (updateState.inProgress) {
      updatePoll = window.setTimeout(pollUpdates, 1500);
    }
  };
  const handleUpdateClick = async () => {
    if (updateState.inProgress) {
      return;
    }
    updateState.events = [];
    updateState.error = null;
    updateState.done = false;
    updateState.updateId = null;
    renderUpdateLog();
    setUpdateStatus("Starting update...");
    setUpdateButtonState();
    try {
      const updateId = await startPeerUpdate(peerId);
      updateState.updateId = updateId;
      updateState.inProgress = true;
      setUpdateButtonState();
      setUpdateStatus("Update started...");
      pollUpdates();
    } catch (error) {
      updateState.error = describeError(error);
      updateState.inProgress = false;
      setUpdateButtonState();
      setUpdateStatus(`Failed to start update: ${updateState.error}`);
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
  updateButton?.addEventListener("click", () => {
    handleUpdateClick();
  });
  setUpdateButtonState();
  renderUpdateLog();
};

// src/treeview.ts
var defaultRow = (node, depth, expanded, hasChildren) => {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "tree-row tree-row--default";
  row.style.setProperty("--tree-depth", String(depth));
  row.setAttribute("data-tree-id", node.id);
  row.innerHTML = `
		<span class="tree-toggle">
			${hasChildren ? `<span class="tree-toggle-btn" data-tree-toggle="${node.id}">${expanded ? "▾" : "▸"}</span>` : `<span class="tree-toggle-placeholder"></span>`}
		</span>
		<span class="tree-body">
			<span class="tree-label">${node.label}</span>
			${node.sublabel ? `<span class="tree-sublabel">${node.sublabel}</span>` : ""}
		</span>
		${node.badge ? `<span class="badge small tree-badge">${node.badge}</span>` : ""}
	`;
  return row;
};
var createTreeView = (options) => {
  let nodes = options.nodes;
  const expanded = options.expanded ?? new Set;
  let nodeById = new Map;
  const root = document.createElement("div");
  root.className = `tree-view${options.className ? ` ${options.className}` : ""}`;
  const buildMap = (list) => {
    nodeById = new Map;
    const walk = (items) => {
      for (const item of items) {
        nodeById.set(item.id, item);
        if (item.children?.length) {
          walk(item.children);
        }
      }
    };
    walk(list);
  };
  const renderNodes = (list, depth) => {
    for (const node of list) {
      const hasChildren = Boolean(node.children && node.children.length > 0);
      const isExpanded = expanded.has(node.id);
      const row = options.renderRow ? options.renderRow(node, depth, isExpanded, hasChildren) : defaultRow(node, depth, isExpanded, hasChildren);
      if (!row.getAttribute("data-tree-id")) {
        row.setAttribute("data-tree-id", node.id);
      }
      row.style.setProperty("--tree-depth", String(depth));
      root.appendChild(row);
      if (hasChildren && isExpanded) {
        renderNodes(node.children, depth + 1);
      }
    }
  };
  const render = () => {
    root.innerHTML = "";
    renderNodes(nodes, 0);
  };
  const toggleNode = (id) => {
    if (expanded.has(id)) {
      expanded.delete(id);
    } else {
      expanded.add(id);
    }
    render();
  };
  root.addEventListener("click", (event) => {
    const target = event.target;
    if (!target)
      return;
    const toggleEl = target.closest("[data-tree-toggle]");
    if (toggleEl) {
      const id2 = toggleEl.getAttribute("data-tree-toggle");
      if (id2) {
        event.stopPropagation();
        toggleNode(id2);
      }
      return;
    }
    const rowEl = target.closest("[data-tree-id]");
    if (!rowEl)
      return;
    const id = rowEl.getAttribute("data-tree-id");
    if (!id)
      return;
    const node = nodeById.get(id);
    if (!node)
      return;
    options.onSelect?.(node);
  });
  buildMap(nodes);
  render();
  return {
    element: root,
    expanded,
    toggle: toggleNode,
    setNodes: (next) => {
      nodes = next;
      buildMap(nodes);
      render();
    }
  };
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
  const tree = createTreeView({
    nodes: [],
    className: "files-tree",
    onSelect: (node) => {
      if (state.loading)
        return;
      if (state.showingDisks) {
        const disk = node.data;
        state.showingDisks = false;
        state.path = disk.mount_path;
        state.entries = [];
        state.error = null;
        loadBrowser();
        return;
      }
      const entry = node.data;
      const target = joinChildPath(state.path, entry.name);
      if (entry.is_dir) {
        state.showingDisks = false;
        state.path = target;
        state.entries = [];
        state.error = null;
        loadBrowser();
        return;
      }
      const params = new URLSearchParams;
      params.set("peer", state.peerId);
      params.set("path", target);
      navigate(`/file?${params.toString()}`);
    }
  });
  if (browserEl) {
    browserEl.appendChild(tree.element);
  }
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
      browserEl.appendChild(tree.element);
      return;
    }
    if (state.error) {
      browserEl.innerHTML = `<p class="muted">Error: ${escapeHtml2(state.error)}</p>`;
      browserEl.appendChild(tree.element);
      tree.setNodes([]);
      return;
    }
    if (state.showingDisks) {
      if (!state.disks.length) {
        browserEl.innerHTML = `<p class="muted">No disks were reported for this peer.</p>`;
        browserEl.appendChild(tree.element);
        tree.setNodes([]);
        return;
      }
      const nodes2 = state.disks.map((disk) => {
        const label = disk.name || disk.mount_path;
        return {
          id: `disk:${disk.mount_path}`,
          label: escapeHtml2(label),
          sublabel: escapeHtml2(`${disk.mount_path} • ${formatSize(disk.available_space)} free of ${formatSize(disk.total_space)}`),
          badge: "disk",
          data: disk
        };
      });
      browserEl.innerHTML = "";
      browserEl.appendChild(tree.element);
      tree.setNodes(nodes2);
      return;
    }
    if (!state.entries.length) {
      browserEl.innerHTML = `<p class="muted">Directory is empty.</p>`;
      browserEl.appendChild(tree.element);
      tree.setNodes([]);
      return;
    }
    const sorted = [...state.entries].sort((a, b) => {
      if (a.is_dir !== b.is_dir)
        return a.is_dir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    const nodes = sorted.map((entry) => {
      const meta = entry.is_dir ? "Directory" : `${entry.mime ?? "File"} • ${formatSize(entry.size)}`;
      return {
        id: joinChildPath(state.path, entry.name),
        label: escapeHtml2(entry.name),
        sublabel: escapeHtml2(meta),
        badge: entry.is_dir ? "dir" : "file",
        data: entry
      };
    });
    browserEl.innerHTML = "";
    browserEl.appendChild(tree.element);
    tree.setNodes(nodes);
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
  const describeError2 = (error) => error instanceof Error ? error.message : String(error);
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
      peerError = describeError2(err);
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
var escapeHtml3 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
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
							<th>Hash</th>
							<th></th>
						</tr>
					</thead>
					<tbody id="search-body"></tbody>
				</table>
			</div>
			<div id="search-sentinel"></div>
		`;
  };
  const formatHashValue = (value) => {
    if (!value)
      return "";
    if (typeof value === "string") {
      return value;
    }
    if (!Array.isArray(value)) {
      return "";
    }
    return value.map((byte) => byte.toString(16).padStart(2, "0")).join("");
  };
  const shortHash = (value) => {
    if (!value)
      return "";
    return value.length > 16 ? `${value.slice(0, 8)}…${value.slice(-8)}` : value;
  };
  const appendRows = (rows) => {
    const body = document.getElementById("search-body");
    if (!body)
      return;
    const html = rows.map((r) => {
      const hash = formatHashValue(r.hash);
      const nodeId = formatHashValue(r.node_id);
      const path = r.path ?? "";
      return `
						<tr>
							<td>${escapeHtml3(r.name ?? "")}</td>
							<td class="muted">${escapeHtml3(r.mime_type ?? "unknown")}</td>
							<td>${((r.size ?? 0) / 1024).toFixed(1)} KB</td>
							<td><span class="badge small">${r.replicas} replicas</span></td>
							<td class="muted">${escapeHtml3(r.latest_datetime ?? "")}</td>
							<td class="muted hash-cell">${escapeHtml3(shortHash(hash))}</td>
							<td>
								<button
									type="button"
									class="link-btn"
									data-hash-link="${escapeHtml3(hash)}"
									data-node-id="${escapeHtml3(nodeId)}"
									data-path="${escapeHtml3(path)}"
								>
									View
								</button>
							</td>
						</tr>
					`;
    }).join("");
    body.insertAdjacentHTML("beforeend", html);
  };
  tableEl?.addEventListener("click", (event) => {
    const target = event.target;
    const button = target?.closest("[data-hash-link]");
    if (!button)
      return;
    const hash = button.getAttribute("data-hash-link");
    if (!hash)
      return;
    const nodeId = button.getAttribute("data-node-id");
    const path = button.getAttribute("data-path");
    const params = new URLSearchParams;
    if (nodeId)
      params.set("node", nodeId);
    if (path)
      params.set("path", path);
    const suffix = params.toString() ? `?${params.toString()}` : "";
    navigate(`/file/${encodeURIComponent(hash)}${suffix}`);
  });
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
var escapeHtml4 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
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
			<div class="storage-view-toggle" role="tablist" aria-label="Storage view">
				<button type="button" id="storage-view-tree" role="tab" aria-selected="true">Tree</button>
				<button type="button" id="storage-view-heatmap" role="tab" aria-selected="false">Heatmap</button>
			</div>
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
  const viewTreeButton = content.querySelector("#storage-view-tree");
  const viewHeatmapButton = content.querySelector("#storage-view-heatmap");
  const state = {
    nodes: [],
    loading: true,
    error: null,
    customStatus: null,
    viewMode: "tree",
    heatmapFocus: new Map
  };
  const heatColor = (percent) => {
    const clamped = Math.max(0, Math.min(100, percent));
    const hue = 210 - clamped * 2.1;
    return `hsl(${hue.toFixed(0)}deg 70% 40%)`;
  };
  const renderHeatmap = () => {
    if (!listEl)
      return;
    if (!state.nodes.length) {
      listEl.innerHTML = `<p class="muted">No storage data available.</p>`;
      return;
    }
    const findEntry = (entries, path) => {
      for (const entry of entries) {
        if (entry.path === path)
          return entry;
        if (entry.children.length) {
          const found = findEntry(entry.children, path);
          if (found)
            return found;
        }
      }
      return null;
    };
    const nodeSections = state.nodes.map((node) => {
      const focusPath = state.heatmapFocus.get(node.id) ?? null;
      const focusEntry = focusPath ? findEntry(node.entries, focusPath) : null;
      const focusEntries = focusEntry ? focusEntry.children : node.entries;
      const tiles = focusEntries.slice(0, 120).map((entry) => {
        const bg = heatColor(entry.percent);
        const percentLabel = `${entry.percent.toFixed(1)}%`;
        return `
						<button
							type="button"
							class="storage-heatmap-tile"
							style="--heat-bg: ${bg}"
							data-entry-open="${escapeHtml4(entry.path)}"
							data-entry-has-children="${entry.children.length ? "1" : "0"}"
							data-node-id="${escapeHtml4(node.id)}"
							title="${escapeHtml4(entry.path)} • ${formatSize2(entry.size)}"
						>
							<strong class="storage-heatmap-name">${escapeHtml4(entry.name)}</strong>
							<span class="storage-heatmap-meta">${percentLabel} • ${formatSize2(entry.size)}</span>
							<span class="storage-heatmap-count">${entry.itemCount} item(s)</span>
						</button>
					`;
      });
      const backButton = focusEntry || focusPath ? `<button type="button" class="link-btn storage-heatmap-back" data-heatmap-back="${escapeHtml4(node.id)}">Back</button>` : "";
      return `
					<section class="storage-heatmap-node">
						<header class="storage-heatmap-header">
							<h3>${escapeHtml4(node.name)}</h3>
							<p class="muted">${escapeHtml4(node.id)} • ${formatSize2(node.totalSize)}</p>
							${backButton}
						</header>
						<div class="storage-heatmap-grid">
							${tiles.join("")}
						</div>
					</section>
				`;
    }).join("");
    listEl.innerHTML = `<div class="storage-heatmap">${nodeSections}</div>`;
    const tileButtons = listEl.querySelectorAll("[data-entry-open]");
    tileButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const path = btn.getAttribute("data-entry-open");
        const nodeId = btn.getAttribute("data-node-id") ?? "";
        const hasChildren = btn.getAttribute("data-entry-has-children") === "1";
        if (!path)
          return;
        if (hasChildren) {
          state.heatmapFocus.set(nodeId, path);
          updateStorageView();
          return;
        }
        state.customStatus = `Selected ${path}`;
        updateStatus();
      });
    });
    const backButtons = listEl.querySelectorAll("[data-heatmap-back]");
    backButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const nodeId = btn.getAttribute("data-heatmap-back") ?? "";
        state.heatmapFocus.set(nodeId, null);
        updateStorageView();
      });
    });
  };
  const buildEntryNodes = (nodeId, entries) => entries.map((entry) => ({
    id: `${nodeId}:${entry.path}`,
    label: entry.name,
    sublabel: entry.path,
    data: entry,
    children: buildEntryNodes(nodeId, entry.children)
  }));
  const renderRow = (node, depth, expanded, hasChildren) => {
    const row = document.createElement("div");
    row.className = "storage-row storage-tree-row";
    row.setAttribute("data-tree-id", node.id);
    row.style.setProperty("--tree-depth", String(depth));
    const data = node.data;
    const isTopNode = data.entries !== undefined;
    if (isTopNode) {
      const storageNode = data;
      row.innerHTML = `
				<div class="storage-cell storage-name storage-tree-name">
					${hasChildren ? `<button type="button" class="link-btn" data-tree-toggle="${node.id}">${expanded ? "▾" : "▸"}</button>` : `<span class="storage-toggle-placeholder"></span>`}
					<div class="storage-name-content">
						<strong>${escapeHtml4(storageNode.name)}</strong>
						<p class="muted storage-node-id">${escapeHtml4(storageNode.id)}</p>
					</div>
				</div>
				<div class="storage-cell">100%</div>
				<div class="storage-cell">${formatSize2(storageNode.totalSize)}</div>
				<div class="storage-cell">-</div>
				<div class="storage-cell muted">-</div>
				<div class="storage-cell"></div>
			`;
      return row;
    }
    const entry = data;
    const openButton = hasChildren ? "" : `<button type="button" class="link-btn" data-entry-open="${escapeHtml4(entry.path)}">Open</button>`;
    row.innerHTML = `
			<div class="storage-cell storage-name storage-tree-name">
				${hasChildren ? `<button type="button" class="link-btn" data-tree-toggle="${node.id}">${expanded ? "▾" : "▸"}</button>` : `<span class="storage-toggle-placeholder"></span>`}
				<div class="storage-name-content">
					<strong>${escapeHtml4(entry.name)}</strong>
					<p class="muted">${escapeHtml4(entry.path)}</p>
				</div>
			</div>
			<div class="storage-cell">${entry.percent.toFixed(1)}%</div>
			<div class="storage-cell">${formatSize2(entry.size)}</div>
			<div class="storage-cell">${entry.itemCount}</div>
			<div class="storage-cell">${formatTimestamp(entry.lastChanged)}</div>
			<div class="storage-cell">${openButton}</div>
		`;
    return row;
  };
  const tree = createTreeView({
    nodes: [],
    className: "storage-tree",
    renderRow,
    onSelect: (node) => {
      const data = node.data;
      if (node.children?.length) {
        tree.toggle(node.id);
        return;
      }
      if (data.path !== undefined) {
        const entry = data;
        state.customStatus = `Selected ${entry.path}`;
        updateStatus();
      }
    }
  });
  if (listEl) {
    listEl.appendChild(tree.element);
  }
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
  const updateStorageView = () => {
    if (!listEl)
      return;
    if (state.loading) {
      listEl.innerHTML = `<p class="muted">Loading storage usage...</p>`;
      listEl.appendChild(tree.element);
      tree.setNodes([]);
    } else if (state.error) {
      const errorMessage = escapeHtml4(state.error ?? "Unknown error");
      listEl.innerHTML = `<p class="muted">Error: ${errorMessage}</p>`;
      listEl.appendChild(tree.element);
      tree.setNodes([]);
    } else if (!state.nodes.length) {
      listEl.innerHTML = `<p class="muted">No storage data available.</p>`;
      listEl.appendChild(tree.element);
      tree.setNodes([]);
    } else {
      if (state.viewMode === "heatmap") {
        renderHeatmap();
      } else {
        const nodes = state.nodes.map((storageNode) => {
          const nodeId = `node:${storageNode.id}`;
          return {
            id: nodeId,
            label: storageNode.name,
            sublabel: storageNode.id,
            data: storageNode,
            children: buildEntryNodes(nodeId, storageNode.entries)
          };
        });
        listEl.innerHTML = "";
        listEl.appendChild(tree.element);
        tree.setNodes(nodes);
      }
    }
    updateStatus();
    if (state.viewMode === "tree") {
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
    }
  };
  const loadStorage = async () => {
    state.loading = true;
    state.error = null;
    state.customStatus = null;
    state.nodes = [];
    state.heatmapFocus.clear();
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
  const setViewMode = (mode) => {
    state.viewMode = mode;
    if (viewTreeButton) {
      viewTreeButton.setAttribute("aria-selected", String(mode === "tree"));
      viewTreeButton.classList.toggle("active", mode === "tree");
    }
    if (viewHeatmapButton) {
      viewHeatmapButton.setAttribute("aria-selected", String(mode === "heatmap"));
      viewHeatmapButton.classList.toggle("active", mode === "heatmap");
    }
    updateStorageView();
  };
  viewTreeButton?.addEventListener("click", () => setViewMode("tree"));
  viewHeatmapButton?.addEventListener("click", () => setViewMode("heatmap"));
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
var escapeHtml5 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
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
			<pre>${escapeHtml5(body)}</pre>
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

// src/pages/file.ts
var formatBytes3 = (value) => {
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
var escapeHtml6 = (value) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
var isTextMime = (mime) => /^(text\/|application\/(json|xml|javascript|svg|x-www-form-urlencoded))/i.test(mime);
var isImageMime = (mime) => /^image\//i.test(mime);
var isVideoMime = (mime) => /^video\//i.test(mime);
var previewLimit = 160000;
var decodeText = (data) => {
  const decoder = new TextDecoder("utf-8", { fatal: false });
  return decoder.decode(data);
};
var createPreviewRenderer = (previewEl, noteEl) => {
  let currentObjectUrl = null;
  const cleanup = () => {
    if (currentObjectUrl) {
      URL.revokeObjectURL(currentObjectUrl);
      currentObjectUrl = null;
    }
  };
  const render = (result, mediaUrl) => {
    if (!previewEl || !noteEl) {
      return;
    }
    cleanup();
    previewEl.classList.remove("file-content--image");
    previewEl.classList.remove("file-content--video");
    if (isImageMime(result.mime)) {
      const blob = new Blob([result.data.buffer], { type: result.mime });
      currentObjectUrl = URL.createObjectURL(blob);
      const img = document.createElement("img");
      img.src = currentObjectUrl;
      img.alt = "Image preview";
      img.loading = "lazy";
      img.decoding = "async";
      previewEl.classList.add("file-content--image");
      previewEl.textContent = "";
      previewEl.appendChild(img);
      noteEl.textContent = "";
      return;
    }
    if (isVideoMime(result.mime)) {
      const sourceUrl = mediaUrl ?? (() => {
        const blob = new Blob([result.data.buffer], { type: result.mime });
        currentObjectUrl = URL.createObjectURL(blob);
        return currentObjectUrl;
      })();
      const video = document.createElement("video");
      const source = document.createElement("source");
      source.src = sourceUrl;
      source.type = result.mime;
      video.appendChild(source);
      video.controls = true;
      video.preload = "metadata";
      video.className = "file-video";
      previewEl.classList.add("file-content--video");
      previewEl.textContent = "";
      previewEl.appendChild(video);
      noteEl.textContent = "";
      return;
    }
    const truncated = result.data.length > previewLimit;
    const chunk = result.data.slice(0, previewLimit);
    if (isTextMime(result.mime)) {
      previewEl.textContent = decodeText(chunk);
      noteEl.textContent = truncated ? "Preview truncated to avoid flooding the UI." : "";
      return;
    }
    const snippet2 = chunk.slice(0, 128);
    const hex = Array.from(snippet2).map((byte) => byte.toString(16).padStart(2, "0")).join(" ");
    previewEl.textContent = hex;
    noteEl.textContent = result.data.length ? `Binary data (${result.mime}); showing first ${snippet2.length} byte(s).` : "No data available.";
  };
  return { render, cleanup };
};
var guessMimeFromPath = (value) => {
  const normalized = value.trim().toLowerCase();
  if (normalized.endsWith(".txt"))
    return "text/plain";
  if (normalized.endsWith(".json"))
    return "application/json";
  if (normalized.endsWith(".md"))
    return "text/markdown";
  if (normalized.endsWith(".csv"))
    return "text/csv";
  if (normalized.endsWith(".html") || normalized.endsWith(".htm"))
    return "text/html";
  if (normalized.endsWith(".xml"))
    return "application/xml";
  if (normalized.endsWith(".js"))
    return "application/javascript";
  if (normalized.endsWith(".png"))
    return "image/png";
  if (normalized.endsWith(".jpg") || normalized.endsWith(".jpeg"))
    return "image/jpeg";
  if (normalized.endsWith(".gif"))
    return "image/gif";
  if (normalized.endsWith(".webp"))
    return "image/webp";
  if (normalized.endsWith(".bmp"))
    return "image/bmp";
  if (normalized.endsWith(".ico"))
    return "image/x-icon";
  if (normalized.endsWith(".mp4") || normalized.endsWith(".m4v"))
    return "video/mp4";
  if (normalized.endsWith(".webm"))
    return "video/webm";
  if (normalized.endsWith(".mov"))
    return "video/quicktime";
  if (normalized.endsWith(".avi"))
    return "video/x-msvideo";
  if (normalized.endsWith(".mkv"))
    return "video/x-matroska";
  return "application/octet-stream";
};
var renderFileByHash = async (hash) => {
  const content = ensureShell(`/file/${hash}`);
  content.innerHTML = `
	<section class="hero">
		<h1>File</h1>
		<p class="lede">Viewing file contents for hash ${escapeHtml6(hash)}</p>
	</section>
	<div class="card" id="file-card">
		<div class="card-heading">
			<h2>File preview</h2>
			<p id="file-status" class="muted">Loading file data…</p>
		</div>
		<div class="file-meta">
			<p><span class="muted">Hash:</span> ${escapeHtml6(hash)}</p>
			<p><span class="muted">Download:</span> <a id="file-download" target="_blank" rel="noreferrer">Raw download</a></p>
			<p><span class="muted">MIME:</span> <span id="file-mime">-</span></p>
			<p><span class="muted">Size:</span> <span id="file-size">-</span></p>
		</div>
		<div class="file-preview">
			<h3>Preview</h3>
			<pre id="file-content" class="resource-meta">Awaiting content…</pre>
			<p id="file-preview-note" class="muted"></p>
		</div>
		<div class="file-actions">
			<button type="button" id="file-refresh">Reload preview</button>
		</div>
	</div>
`;
  const statusEl = document.getElementById("file-status");
  const previewEl = document.getElementById("file-content");
  const noteEl = document.getElementById("file-preview-note");
  const downloadLink = document.getElementById("file-download");
  const mimeEl = document.getElementById("file-mime");
  const sizeEl = document.getElementById("file-size");
  const refreshButton = document.getElementById("file-refresh");
  const params = new URLSearchParams(window.location.search);
  const remoteNodeId = params.get("node");
  const remotePath = params.get("path");
  const downloadUrl = `${getServerAddr()}/api/file/hash?hash=${encodeURIComponent(hash)}`;
  const updateDownloadLink = (url) => {
    if (!downloadLink)
      return;
    if (url) {
      downloadLink.href = url;
    } else {
      downloadLink.removeAttribute("href");
    }
  };
  updateDownloadLink(downloadUrl);
  const preview = createPreviewRenderer(previewEl, noteEl);
  const loadFile = async () => {
    if (statusEl)
      statusEl.textContent = "Loading file data…";
    if (previewEl)
      previewEl.textContent = "";
    if (noteEl)
      noteEl.textContent = "";
    preview.cleanup();
    try {
      const result = await fetchFileByHash(hash);
      if (mimeEl)
        mimeEl.textContent = result.mime;
      if (sizeEl)
        sizeEl.textContent = formatBytes3(result.length);
      if (statusEl) {
        statusEl.textContent = `Loaded ${formatBytes3(result.length)} (${result.mime})`;
      }
      preview.render(result, downloadUrl);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (remoteNodeId && remotePath) {
        const handled = await loadRemoteFallback();
        if (handled) {
          return;
        }
      }
      if (statusEl)
        statusEl.textContent = `Failed to load file: ${message}`;
      if (previewEl)
        previewEl.textContent = "";
      if (noteEl)
        noteEl.textContent = "";
    }
  };
  const loadRemoteFallback = async () => {
    if (!remoteNodeId || !remotePath) {
      return false;
    }
    try {
      const state = await fetchState();
      const peer = state.peers.find((entry) => entry.node_id === remoteNodeId);
      if (!peer) {
        return false;
      }
      const chunk = await fetchPeerFileChunk(peer.id, remotePath, previewLimit);
      const data = new Uint8Array(chunk.data);
      const remoteResult = {
        data,
        mime: guessMimeFromPath(remotePath),
        length: data.length,
        status: chunk.eof ? 200 : 206
      };
      if (mimeEl)
        mimeEl.textContent = remoteResult.mime;
      if (sizeEl)
        sizeEl.textContent = formatBytes3(remoteResult.length);
      if (statusEl) {
        statusEl.textContent = `Loaded ${formatBytes3(remoteResult.length)} (${peer.name ?? peer.id})`;
      }
      const remoteUrl = `${getServerAddr()}/api/peers/${encodeURIComponent(peer.id)}/file?path=${encodeURIComponent(remotePath)}`;
      preview.render(remoteResult, remoteUrl);
      updateDownloadLink(remoteUrl);
      if (noteEl) {
        noteEl.textContent = `Remote path: ${escapeHtml6(remotePath)}`;
      }
      return true;
    } catch {
      return false;
    }
  };
  refreshButton?.addEventListener("click", () => {
    loadFile();
  });
  loadFile();
};
var renderFileByPath = async (peerId, path) => {
  const content = ensureShell("/file");
  content.innerHTML = `
	<section class="hero">
		<h1>File</h1>
		<p class="lede">Viewing file contents for ${escapeHtml6(path)}</p>
	</section>
	<div class="card" id="file-card">
		<div class="card-heading">
			<h2>File preview</h2>
			<p id="file-status" class="muted">Loading file data…</p>
		</div>
		<div class="file-meta">
			<p><span class="muted">Path:</span> ${escapeHtml6(path)}</p>
			<p><span class="muted">Download:</span> <a id="file-download" target="_blank" rel="noreferrer">Raw download</a></p>
			<p><span class="muted">MIME:</span> <span id="file-mime">-</span></p>
			<p><span class="muted">Size:</span> <span id="file-size">-</span></p>
		</div>
		<div class="file-preview">
			<h3>Preview</h3>
			<pre id="file-content" class="resource-meta">Awaiting content…</pre>
			<p id="file-preview-note" class="muted"></p>
		</div>
		<div class="file-actions">
			<button type="button" id="file-refresh">Reload preview</button>
		</div>
	</div>
`;
  const statusEl = document.getElementById("file-status");
  const previewEl = document.getElementById("file-content");
  const noteEl = document.getElementById("file-preview-note");
  const downloadLink = document.getElementById("file-download");
  const mimeEl = document.getElementById("file-mime");
  const sizeEl = document.getElementById("file-size");
  const refreshButton = document.getElementById("file-refresh");
  const updateDownloadLink = (url) => {
    if (!downloadLink)
      return;
    if (url) {
      downloadLink.href = url;
    } else {
      downloadLink.removeAttribute("href");
    }
  };
  const preview = createPreviewRenderer(previewEl, noteEl);
  const loadFile = async () => {
    if (statusEl)
      statusEl.textContent = "Loading file data…";
    if (previewEl)
      previewEl.textContent = "";
    if (noteEl)
      noteEl.textContent = "";
    preview.cleanup();
    try {
      const chunk = await fetchPeerFileChunk(peerId, path, previewLimit);
      const data = new Uint8Array(chunk.data);
      const result = {
        data,
        mime: guessMimeFromPath(path),
        length: data.length,
        status: chunk.eof ? 200 : 206
      };
      if (mimeEl)
        mimeEl.textContent = result.mime;
      if (sizeEl)
        sizeEl.textContent = formatBytes3(result.length);
      if (statusEl) {
        statusEl.textContent = `Loaded ${formatBytes3(result.length)} (${peerId})`;
      }
      const downloadUrl = `${getServerAddr()}/api/peers/${encodeURIComponent(peerId)}/file?path=${encodeURIComponent(path)}`;
      updateDownloadLink(downloadUrl);
      preview.render(result, downloadUrl);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (statusEl)
        statusEl.textContent = `Failed to load file: ${message}`;
    }
  };
  refreshButton?.addEventListener("click", () => {
    loadFile();
  });
  loadFile();
};

// src/PuppyTerm.ts
class PuppyTerm {
  onData = () => {};
  fontSize;
  fontFamily;
  cursorAlpha;
  defaultFg;
  defaultBg;
  canvas = null;
  ctx = null;
  cols;
  rows;
  charW = 10;
  charH;
  fg;
  bg;
  cx = 0;
  cy = 0;
  buf = [];
  esc = false;
  csi = false;
  csiBuf = "";
  constructor(opts = {}) {
    this.fontSize = opts.fontSize ?? 16;
    this.fontFamily = opts.fontFamily ?? "ui-monospace, Menlo, Consolas, monospace";
    this.cursorAlpha = opts.cursorAlpha ?? 0.35;
    this.defaultFg = opts.defaultFg ?? "#ddd";
    this.defaultBg = opts.defaultBg ?? "#111";
    this.fg = this.defaultFg;
    this.bg = this.defaultBg;
    this.cols = opts.cols ?? 80;
    this.rows = opts.rows ?? 24;
    this.charH = Math.ceil(this.fontSize * 1.3);
    this.initBuffer();
  }
  open(canvas) {
    this.canvas = canvas;
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx)
      throw new Error("PuppyTerm: failed to get 2D context");
    this.ctx = ctx;
    ctx.font = `${this.fontSize}px ${this.fontFamily}`;
    ctx.textBaseline = "top";
    this.charW = Math.ceil(ctx.measureText("M").width);
    this.charH = Math.ceil(this.fontSize * 1.3);
    this.cols = Math.max(1, Math.floor(canvas.width / this.charW));
    this.rows = Math.max(1, Math.floor(canvas.height / this.charH));
    this.initBuffer();
    this.clear();
    this.render();
    this.installInput();
  }
  resizeToCanvas() {
    if (!this.canvas || !this.ctx)
      return;
    const newCols = Math.max(1, Math.floor(this.canvas.width / this.charW));
    const newRows = Math.max(1, Math.floor(this.canvas.height / this.charH));
    if (newCols === this.cols && newRows === this.rows)
      return;
    const old = this.buf;
    const oldCols = this.cols;
    const oldRows = this.rows;
    this.cols = newCols;
    this.rows = newRows;
    this.initBuffer();
    const minRows = Math.min(oldRows, this.rows);
    const minCols = Math.min(oldCols, this.cols);
    for (let y = 0;y < minRows; y++) {
      for (let x = 0;x < minCols; x++) {
        this.buf[y][x] = old[y][x];
      }
    }
    this.cx = Math.min(this.cx, this.cols - 1);
    this.cy = Math.min(this.cy, this.rows - 1);
    this.render();
  }
  write(data) {
    for (let i = 0;i < data.length; i++) {
      const ch = data[i];
      if (this.handleAnsiChar(ch))
        continue;
      if (ch === `
`) {
        this.lineFeed();
        continue;
      }
      if (ch === "\r") {
        this.cx = 0;
        continue;
      }
      if (ch === "\b") {
        this.cx = Math.max(0, this.cx - 1);
        this.putChar(" ");
        continue;
      }
      if (ch === "\t") {
        this.tab();
        continue;
      }
      this.putChar(ch);
    }
    this.render();
  }
  clear() {
    for (let y = 0;y < this.rows; y++) {
      for (let x = 0;x < this.cols; x++) {
        this.buf[y][x] = { ch: " ", fg: this.fg, bg: this.bg };
      }
    }
    this.cx = 0;
    this.cy = 0;
  }
  initBuffer() {
    const makeCell = () => ({ ch: " ", fg: this.fg, bg: this.bg });
    this.buf = Array.from({ length: this.rows }, () => Array.from({ length: this.cols }, makeCell));
  }
  render() {
    const ctx = this.ctx;
    if (!ctx)
      return;
    for (let y = 0;y < this.rows; y++) {
      for (let x = 0;x < this.cols; x++) {
        const cell = this.buf[y][x];
        const px = x * this.charW;
        const py = y * this.charH;
        ctx.fillStyle = cell.bg;
        ctx.fillRect(px, py, this.charW, this.charH);
        if (cell.ch !== " ") {
          ctx.fillStyle = cell.fg;
          ctx.fillText(cell.ch, px, py);
        }
      }
    }
    ctx.globalAlpha = this.cursorAlpha;
    ctx.fillStyle = "#fff";
    ctx.fillRect(this.cx * this.charW, this.cy * this.charH, this.charW, this.charH);
    ctx.globalAlpha = 1;
  }
  putChar(ch) {
    if (this.cx >= this.cols) {
      this.cx = 0;
      this.lineFeed();
    }
    if (this.cy >= this.rows) {
      this.scrollUp();
      this.cy = this.rows - 1;
    }
    this.buf[this.cy][this.cx] = { ch, fg: this.fg, bg: this.bg };
    this.cx++;
  }
  lineFeed() {
    this.cy++;
    if (this.cy >= this.rows) {
      this.scrollUp();
      this.cy = this.rows - 1;
    }
  }
  scrollUp() {
    this.buf.shift();
    const emptyRow = Array.from({ length: this.cols }, () => ({
      ch: " ",
      fg: this.fg,
      bg: this.bg
    }));
    this.buf.push(emptyRow);
  }
  tab() {
    const next = (Math.floor(this.cx / 8) + 1) * 8;
    this.cx = Math.min(next, this.cols - 1);
  }
  handleAnsiChar(ch) {
    if (!this.esc) {
      if (ch === "\x1B") {
        this.esc = true;
        return true;
      }
      return false;
    }
    if (!this.csi) {
      if (ch === "[") {
        this.csi = true;
        this.csiBuf = "";
        return true;
      }
      this.esc = false;
      return true;
    }
    this.csiBuf += ch;
    const code = ch.charCodeAt(0);
    const isFinal = code >= 64 && code <= 126;
    if (!isFinal)
      return true;
    const final = ch;
    const body = this.csiBuf.slice(0, -1);
    this.applyCsi(final, body);
    this.esc = false;
    this.csi = false;
    this.csiBuf = "";
    return true;
  }
  applyCsi(final, body) {
    const isPrivate = body.startsWith("?");
    const paramsStr = isPrivate ? body.slice(1) : body;
    const params = paramsStr.length ? paramsStr.split(";").map((s) => s === "" ? Number.NaN : Number(s)) : [];
    const p = (idx, def) => Number.isFinite(params[idx]) ? params[idx] : def;
    switch (final) {
      case "m":
        if (params.length === 0) {
          this.sgr(0);
          break;
        }
        for (const n of params) {
          this.sgr(Number.isFinite(n) ? n : 0);
        }
        break;
      case "H":
      case "f": {
        const row = p(0, 1);
        const col = p(1, 1);
        this.cy = Math.min(this.rows - 1, Math.max(0, row - 1));
        this.cx = Math.min(this.cols - 1, Math.max(0, col - 1));
        break;
      }
      case "A":
        this.cy = Math.max(0, this.cy - p(0, 1));
        break;
      case "B":
        this.cy = Math.min(this.rows - 1, this.cy + p(0, 1));
        break;
      case "C":
        this.cx = Math.min(this.cols - 1, this.cx + p(0, 1));
        break;
      case "D":
        this.cx = Math.max(0, this.cx - p(0, 1));
        break;
      case "J": {
        const mode = p(0, 0);
        if (mode === 2) {
          this.clear();
        } else if (mode === 0) {
          for (let y = this.cy;y < this.rows; y++) {
            const startX = y === this.cy ? this.cx : 0;
            for (let x = startX;x < this.cols; x++) {
              this.buf[y][x] = { ch: " ", fg: this.fg, bg: this.bg };
            }
          }
        }
        break;
      }
      case "K": {
        const mode = p(0, 0);
        if (mode === 2) {
          for (let x = 0;x < this.cols; x++) {
            this.buf[this.cy][x] = { ch: " ", fg: this.fg, bg: this.bg };
          }
        } else if (mode === 0) {
          for (let x = this.cx;x < this.cols; x++) {
            this.buf[this.cy][x] = { ch: " ", fg: this.fg, bg: this.bg };
          }
        }
        break;
      }
      default:
        break;
    }
  }
  sgr(n) {
    if (n === 0) {
      this.fg = this.defaultFg;
      this.bg = this.defaultBg;
      return;
    }
    const basic = [
      "#000",
      "#a00",
      "#0a0",
      "#aa0",
      "#00a",
      "#a0a",
      "#0aa",
      "#aaa"
    ];
    const bright = [
      "#555",
      "#f55",
      "#5f5",
      "#ff5",
      "#55f",
      "#f5f",
      "#5ff",
      "#fff"
    ];
    if (n >= 30 && n <= 37) {
      const next = basic[n - 30];
      if (next)
        this.fg = next;
    }
    if (n >= 40 && n <= 47) {
      const next = basic[n - 40];
      if (next)
        this.bg = next;
    }
    if (n >= 90 && n <= 97) {
      const next = bright[n - 90];
      if (next)
        this.fg = next;
    }
    if (n >= 100 && n <= 107) {
      const next = bright[n - 100];
      if (next)
        this.bg = next;
    }
  }
  installInput() {
    const canvas = this.canvas;
    canvas.tabIndex = 0;
    canvas.style.outline = "none";
    canvas.addEventListener("mousedown", () => canvas.focus());
    canvas.addEventListener("keydown", (e) => {
      if (["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab"].includes(e.key)) {
        e.preventDefault();
      }
      const data = this.keyToData(e);
      if (data)
        this.onData(data);
    });
  }
  keyToData(e) {
    if (e.key === "Enter")
      return "\r";
    if (e.key === "Backspace")
      return "";
    if (e.key === "Tab")
      return "\t";
    if (e.key === "ArrowUp")
      return "\x1B[A";
    if (e.key === "ArrowDown")
      return "\x1B[B";
    if (e.key === "ArrowRight")
      return "\x1B[C";
    if (e.key === "ArrowLeft")
      return "\x1B[D";
    if (e.ctrlKey && e.key.length === 1) {
      const k = e.key.toUpperCase();
      const code = k.charCodeAt(0);
      if (code >= 65 && code <= 90)
        return String.fromCharCode(code - 64);
    }
    if (e.altKey && e.key.length === 1)
      return "\x1B" + e.key;
    if (!e.ctrlKey && !e.metaKey && e.key.length === 1)
      return e.key;
    return "";
  }
}

// src/pages/peer-shell.ts
var renderPeerShell = async (peerId) => {
  const content = ensureShell("/peers");
  content.innerHTML = `
	<section class="hero">
		<h1>Remote shell</h1>
		<p class="lede">Interactive session for peer ${peerId}</p>
	</section>
	<div class="card">
		<canvas id="peer-shell-canvas" class="peer-shell-canvas"></canvas>
		<p id="peer-shell-status" class="muted" style="margin-top: 8px;">Connecting…</p>
	</div>
`;
  const canvas = content.querySelector("#peer-shell-canvas");
  const statusEl = content.querySelector("#peer-shell-status");
  if (!canvas) {
    throw new Error("shell canvas missing");
  }
  const fitCanvas = () => {
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(300, Math.floor(rect.width * dpr));
    canvas.height = Math.max(240, Math.floor(rect.height * dpr));
  };
  fitCanvas();
  const term = new PuppyTerm;
  term.open(canvas);
  term.write(`Connecting to peer…\r
`);
  let sessionId = null;
  try {
    sessionId = await startPeerShell(peerId);
    if (statusEl)
      statusEl.textContent = `Shell started (session ${sessionId})`;
    term.write(`Connected.\r
`);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    if (statusEl)
      statusEl.textContent = `Failed to start shell: ${msg}`;
    term.write(`Failed to start shell: ${msg}\r
`);
    return;
  }
  const encoder = new TextEncoder;
  const decoder = new TextDecoder("utf-8", { fatal: false });
  let inFlight = false;
  const send = async (data) => {
    if (sessionId === null || inFlight)
      return;
    inFlight = true;
    try {
      const bytes = Array.from(encoder.encode(data));
      const outBytes = await sendPeerShellInput(peerId, sessionId, bytes);
      if (outBytes.length) {
        term.write(decoder.decode(new Uint8Array(outBytes)));
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      if (statusEl)
        statusEl.textContent = `Shell error: ${msg}`;
      term.write(`\r
Shell error: ${msg}\r
`);
    } finally {
      inFlight = false;
    }
  };
  term.onData = (data) => {
    term.write(data);
    send(data);
  };
  window.addEventListener("resize", () => {
    fitCanvas();
    term.resizeToCanvas();
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
    "/peers/:peerId/shell": ({ peerId }) => renderPeerShell(peerId),
    "/user": () => renderUsers(),
    "/user/:userId": ({ userId }) => renderUserDetail(userId),
    "/files": () => renderFiles(),
    "/search": () => renderSearch(),
    "/storage": () => renderStorage(),
    "/updates": () => renderUpdates(),
    "/settings": () => renderSettings(),
    "/file": () => {
      const params = new URLSearchParams(window.location.search);
      const peerId = params.get("peer") ?? "";
      const path = params.get("path") ?? "";
      return renderFileByPath(peerId, path);
    },
    "/file/:hash": ({ hash }) => renderFileByHash(hash),
    "/*": () => renderHome()
  });
  console.info("Using server address:", serverAddr2);
};
