const demoCategories = [
  { id: "office", name: "办公软件", count: 4, path: "AppManager/Apps/办公软件" },
  { id: "dev", name: "开发工具", count: 7, path: "AppManager/Apps/开发工具" },
  { id: "design", name: "设计工具", count: 5, path: "AppManager/Apps/设计工具" },
  { id: "system", name: "系统工具", count: 3, path: "AppManager/Apps/系统工具" }
];

const demoApps = [
  { id: "1", name: "Visual Studio Code", categoryId: "dev", categoryName: "开发工具", note: "代码编辑器", favorite: true, initials: "VS", folderPath: "AppManager/Apps/开发工具/VSCode", executablePath: "" },
  { id: "2", name: "Git Bash", categoryId: "dev", categoryName: "开发工具", note: "版本管理", favorite: true, initials: "GB", folderPath: "AppManager/Apps/开发工具/Git", executablePath: "" },
  { id: "3", name: "Python Tools", categoryId: "dev", categoryName: "开发工具", note: "脚本环境", favorite: false, initials: "PY", folderPath: "AppManager/Apps/开发工具/Python", executablePath: "" },
  { id: "4", name: "Figma", categoryId: "design", categoryName: "设计工具", note: "界面设计", favorite: true, initials: "FI", folderPath: "AppManager/Apps/设计工具/Figma", executablePath: "" },
  { id: "5", name: "Photoshop", categoryId: "design", categoryName: "设计工具", note: "图像编辑", favorite: false, initials: "PS", folderPath: "AppManager/Apps/设计工具/Photoshop", executablePath: "" },
  { id: "6", name: "Word", categoryId: "office", categoryName: "办公软件", note: "文档编辑", favorite: true, initials: "W", folderPath: "AppManager/Apps/办公软件/Word", executablePath: "" },
  { id: "7", name: "Excel", categoryId: "office", categoryName: "办公软件", note: "表格处理", favorite: false, initials: "X", folderPath: "AppManager/Apps/办公软件/Excel", executablePath: "" },
  { id: "8", name: "PowerPoint", categoryId: "office", categoryName: "办公软件", note: "演示文稿", favorite: false, initials: "P", folderPath: "AppManager/Apps/办公软件/PowerPoint", executablePath: "" },
  { id: "9", name: "Everything", categoryId: "system", categoryName: "系统工具", note: "文件搜索", favorite: true, initials: "EV", folderPath: "AppManager/Apps/系统工具/Everything", executablePath: "" },
  { id: "10", name: "7-Zip", categoryId: "system", categoryName: "系统工具", note: "压缩工具", favorite: false, initials: "7Z", folderPath: "AppManager/Apps/系统工具/7-Zip", executablePath: "" }
];

const accents = ["blue", "orange", "green", "pink", "indigo", "slate", "teal"];
const ENABLE_DEBUG_LOGS = false;
const CONNECTION_STATUS_INTERVAL_MS = 15000;
const APP_GRID_RENDER_LIMIT = 240;
const REMOTE_LIST_RENDER_LIMIT = 160;
const REVIEW_LIST_RENDER_LIMIT = 120;
const SLOW_RENDER_THRESHOLD_MS = 80;

const state = {
  view: "favorites",
  query: "",
  theme: "light",
  density: "comfortable",
  modal: null,
  selectedAppId: null,
  contextMenu: null,
  creatingCategory: false,
  toast: "",
  loading: false,
  libraryPath: "AppManagerLibrary",
  runMode: "local",
  autostartEnabled: false,
  serverHost: "0.0.0.0",
  serverPort: 8765,
  serverUsername: "admin",
  serverPassword: "",
  serverAllowDownloads: true,
  serverStatus: null,
  packageCache: { path: "", fileCount: 0, totalSize: 0 },
  clientStatus: null,
  clientHost: "127.0.0.1",
  clientPort: 8765,
  clientUsername: "admin",
  clientPassword: "",
  remoteApps: [],
  uploadQueue: [],
  reviewApps: [],
  transfers: {},
  transferDebug: {},
  transferUnlisten: null,
  suppressLaunchUntil: 0,
  dragFavoriteId: null,
  dragFavoriteTargetId: null,
  favoriteOrder: [],
  categories: demoCategories,
  apps: demoApps,
  isTauri: Boolean(window.__TAURI__?.core?.invoke)
};

const app = document.querySelector("#app");
let renderedView = null;
let transferRenderTimer = null;
let toastTimer = null;
let connectionStatusTimer = null;
let connectionStatusRefreshing = false;
let renderSequence = 0;
const transferPollers = {};

function invoke(command, payload) {
  return window.__TAURI__.core.invoke(command, payload);
}

function isLocalMode() {
  return state.runMode === "local";
}

function isServerMode() {
  return state.runMode === "server";
}

function isClientMode() {
  return state.runMode === "client";
}

async function boot() {
  render();

  if (state.isTauri) {
    try {
      await setupTransferListener();
    } catch (error) {
      debugLog(`transfer listener unavailable error=${formatDebugError(error)}`);
    }
  }

  if (!state.isTauri) {
    return;
  }

  await runTask(async () => {
    const result = await invoke("init_library");
    applyData(result.data);
    state.libraryPath = result.libraryPath;
    state.theme = normalizeTheme(result.data.settings?.theme ?? state.theme);
    applyTheme();
    state.density = result.data.settings?.gridDensity ?? "comfortable";
    state.runMode = result.data.settings?.runMode ?? "local";
    state.autostartEnabled = Boolean(result.data.settings?.autostartEnabled);
    state.serverStatus = isServerMode() && state.isTauri ? await invoke("get_server_status") : null;
    state.packageCache = state.isTauri ? await invoke("get_package_cache_info") : state.packageCache;
    state.clientStatus = isClientMode() ? idleClientStatus() : inactiveClientStatus();
    if (isServerMode()) {
      await refreshReviewApps({ silent: true });
    }
  }, "软件库初始化失败");

  render();
  startConnectionStatusPolling();
}

async function setupTransferListener() {
  debugLog(`transfer listener setup start hasWindowTauri=${Boolean(window.__TAURI__)} keys=${Object.keys(window.__TAURI__ || {}).join("|")}`);
  let listen = window.__TAURI__?.event?.listen;
  debugLog(`transfer listener window api type=${typeof listen} hasEvent=${Boolean(window.__TAURI__?.event)}`);
  if (!listen) {
    try {
      debugLog("transfer listener dynamic import start");
      const eventApi = await import("@tauri-apps/api/event");
      listen = eventApi.listen;
      debugLog(`transfer listener dynamic import ok keys=${Object.keys(eventApi).join("|")} listenType=${typeof listen}`);
    } catch (error) {
      debugLog(`transfer listener import failed error=${formatDebugError(error)}`);
      return;
    }
  }
  if (!listen) {
    debugLog("transfer listener unavailable");
    return;
  }

  try {
    debugLog("transfer listener listen call start");
    let pending = true;
    window.setTimeout(() => {
      if (pending) debugLog("transfer listener listen still pending after 3000ms");
    }, 3000);
    state.transferUnlisten = await listen("transfer-progress", (event) => {
      applyTransferProgress(event.payload);
    });
    pending = false;
    debugLog(`transfer listener ready unlistenType=${typeof state.transferUnlisten}`);
  } catch (error) {
    debugLog(`transfer listener listen failed error=${formatDebugError(error)}`);
  }
}

function debugLog(message) {
  if (!ENABLE_DEBUG_LOGS) return;
  if (!state.isTauri) return;
  invoke("debug_log", { message }).catch(() => {});
}

function memoryDebugText() {
  const memory = performance?.memory;
  if (!memory) return "memory=unavailable";
  return `memory=${formatBytes(memory.usedJSHeapSize || 0)}/${formatBytes(memory.totalJSHeapSize || 0)}`;
}

function formatDebugError(error) {
  if (!error) return "unknown";
  if (typeof error === "string") return error;
  return error.stack || error.message || String(error);
}

function debugTransferEvent(key, progress) {
  const now = Date.now();
  const previous = state.transferDebug[key] || { lastLogAt: 0, lastTransferred: 0 };
  const transferred = Number(progress.transferred || 0);
  const total = Number(progress.total || 0);
  const shouldLog = progress.status !== "running"
    || transferred === 0
    || transferred === total
    || now - previous.lastLogAt > 10000
    || transferred - previous.lastTransferred >= 100 * 1024 * 1024;

  if (!shouldLog) return;
  state.transferDebug[key] = { lastLogAt: now, lastTransferred: transferred };
  debugLog(`transfer event key=${key} status=${progress.status} transferred=${transferred} total=${total} percent=${progress.percent}`);
}

function scheduleTransferRender(progress) {
  if (updateTransferElement(progress)) return;

  if (["done", "error", "extracting", "installing"].includes(progress.status)) {
    window.clearTimeout(transferRenderTimer);
    transferRenderTimer = null;
    renderContent();
    return;
  }

  if (transferRenderTimer) return;
  transferRenderTimer = window.setTimeout(() => {
    transferRenderTimer = null;
    renderContent();
  }, 250);
}

function applyTransferProgress(progress) {
  if (!progress?.direction || !progress?.appId) return;
  const key = `${progress.direction}-${progress.appId}`;
  debugTransferEvent(key, progress);
  state.transfers[key] = {
    ...(state.transfers[key] || {}),
    ...progress
  };
  const created = ensureTransferListItem(progress);
  if (progress.direction === "upload" && progress.appId.startsWith("server-upload-")) {
    if (progress.status === "done") refreshServerUploadData();
    if (created) {
      render();
      return;
    }
  }
  scheduleTransferRender(progress);
}

function ensureTransferListItem(progress) {
  if (progress.direction !== "upload") return false;
  if (state.uploadQueue.some((item) => item.id === progress.appId)) return false;
  state.uploadQueue = [
    {
      id: progress.appId,
      name: progress.appName || "\u4e0a\u4f20\u8f6f\u4ef6",
      categoryName: "\u672a\u5ba1\u6838\u8f6f\u4ef6",
      note: "\u6b63\u5728\u63a5\u6536\u5ba2\u6237\u7aef\u4e0a\u4f20",
      iconDataUrl: ""
    },
    ...state.uploadQueue
  ];
  if (progress.appId.startsWith("server-upload-")) {
    state.view = "remote";
  }
  return true;
}

function startTransferPolling(direction, appId) {
  const key = `${direction}-${appId}`;
  stopTransferPolling(key);
  transferPollers[key] = window.setInterval(async () => {
    try {
      const progress = await invoke("get_transfer_progress", { direction, appId });
      if (progress) {
        applyTransferProgress(progress);
        if (["done", "error"].includes(progress.status)) {
          stopTransferPolling(key);
        }
      }
    } catch (error) {
      debugLog(`transfer poll failed key=${key} error=${error}`);
    }
  }, 800);
}

function stopTransferPolling(key) {
  if (!transferPollers[key]) return;
  window.clearInterval(transferPollers[key]);
  delete transferPollers[key];
}

function applyData(data) {
  const counts = data.apps.reduce((map, item) => {
    map[item.categoryId] = (map[item.categoryId] || 0) + 1;
    return map;
  }, {});

  state.categories = data.categories.map((item) => ({
    ...item,
    count: counts[item.id] ?? 0
  }));

  state.apps = data.apps.map((item, index) => ({
    ...item,
    initials: getInitials(item.name),
    accent: accents[index % accents.length],
    iconDataUrl: item.iconDataUrl || "",
    executableCandidates: item.executableCandidates || [],
    note: item.note || (item.executablePath ? "已识别启动程序" : "需要选择启动程序")
  }));

  state.theme = normalizeTheme(data.settings?.theme ?? state.theme);
  applyTheme();
  state.density = data.settings?.gridDensity ?? state.density;
  state.runMode = data.settings?.runMode ?? state.runMode;
  state.autostartEnabled = Boolean(data.settings?.autostartEnabled);
  state.serverHost = data.settings?.server?.host ?? state.serverHost;
  state.serverPort = data.settings?.server?.port ?? state.serverPort;
  state.serverUsername = data.settings?.server?.username ?? state.serverUsername;
  state.serverPassword = data.settings?.server?.password ?? state.serverPassword;
  state.serverAllowDownloads = data.settings?.server?.allowDownloads ?? state.serverAllowDownloads;
  state.favoriteOrder = data.settings?.favoriteOrder || [];
  state.clientHost = data.settings?.client?.host ?? state.clientHost;
  state.clientPort = data.settings?.client?.port ?? state.clientPort;
  state.clientUsername = data.settings?.client?.username ?? state.clientUsername;
  state.clientPassword = data.settings?.client?.password ?? state.clientPassword;
}

function inactiveClientStatus() {
  return {
    configured: false,
    online: false,
    host: state.clientHost,
    port: state.clientPort,
    username: state.clientUsername,
    message: isServerMode()
      ? "当前为服务端模式，客户端连接功能未启用"
      : "当前为本地模式，远程连接功能未启用",
    checkedAt: Math.floor(Date.now() / 1000),
    allowDownloads: null
  };
}

function idleClientStatus(message = "未检测") {
  return {
    configured: Boolean(state.clientHost && state.clientUsername && state.clientPassword),
    online: false,
    host: state.clientHost,
    port: state.clientPort,
    username: state.clientUsername,
    message,
    checkedAt: null,
    allowDownloads: null
  };
}

function inactiveServerStatus() {
  return {
    running: false,
    host: "",
    port: 0,
    clients: []
  };
}

function getVisibleApps() {
  if (state.view === "settings") return [];
  if (state.view === "remote") {
    const query = state.query.trim().toLowerCase();
    return state.remoteApps.filter((item) => {
      return !query
        || item.name.toLowerCase().includes(query)
        || item.categoryName.toLowerCase().includes(query)
        || String(item.note || "").toLowerCase().includes(query);
    });
  }

  const query = state.query.trim().toLowerCase();
  const items = state.apps.filter((item) => {
    const viewMatch =
      state.view === "favorites"
        ? item.favorite
        : state.view === "all"
          ? true
          : item.categoryId === state.view;

    const queryMatch = !query
      || item.name.toLowerCase().includes(query)
      || item.categoryName.toLowerCase().includes(query)
      || item.note.toLowerCase().includes(query);

    return viewMatch && queryMatch;
  });

  if (state.view === "favorites") {
    return sortFavorites(items);
  }

  return items;
}

function sortFavorites(items) {
  const order = new Map((state.favoriteOrder || []).map((id, index) => [id, index]));
  return [...items].sort((left, right) => {
    const leftOrder = order.has(left.id) ? order.get(left.id) : Number.MAX_SAFE_INTEGER;
    const rightOrder = order.has(right.id) ? order.get(right.id) : Number.MAX_SAFE_INTEGER;
    if (leftOrder !== rightOrder) return leftOrder - rightOrder;
    return left.name.localeCompare(right.name, "zh-Hans-CN");
  });
}

function normalizeTheme(value) {
  return ["light", "dark", "green"].includes(value) ? value : "light";
}

function themeLabel(value) {
  return {
    light: "浅色",
    dark: "深色",
    green: "青绿"
  }[normalizeTheme(value)];
}

function applyTheme() {
  document.documentElement.dataset.theme = normalizeTheme(state.theme);
}

function getTitle() {
  if (state.view === "favorites") return "常用软件";
  if (state.view === "all") return "全部软件";
  if (state.view === "settings") return "设置";
  if (state.view === "remote") return "远程软件";
  return state.categories.find((item) => item.id === state.view)?.name ?? "软件";
}

function render() {
  const renderId = ++renderSequence;
  const startedAt = performance.now();
  const beforeNodeCount = document.getElementsByTagName("*").length;
  const contentElement = document.querySelector(".content");
  const preserveContentScroll = renderedView === state.view && state.view === "settings";
  const previousContentScroll = preserveContentScroll ? contentElement?.scrollTop ?? 0 : 0;
  const visibleApps = getVisibleApps();
  const title = getTitle();
  const isSettingsView = state.view === "settings";
  const isRemoteView = state.view === "remote";
  const remoteCount = getRemoteNavCount();

  app.innerHTML = `
    <main class="shell">
      <aside class="sidebar">
        <div class="brand">
          <div class="brand-mark">A</div>
          <div>
            <div class="brand-title">AppManager</div>
            <div class="brand-subtitle">软件启动中心</div>
          </div>
        </div>

        <nav class="nav-section">
          ${navItem("favorites", "☆", "常用软件", state.apps.filter((item) => item.favorite).length)}
          ${navItem("all", "◎", "全部软件", state.apps.length)}
          ${navItem("remote", "⇄", "远程连接", remoteCount)}
        </nav>

        <div class="section-label">分类</div>
        <nav class="nav-section category-list">
          ${state.categories.map((item) => navItem(item.id, "□", item.name, item.count)).join("")}
        </nav>

        ${state.creatingCategory ? `
          <label class="inline-category">
            <span>＋</span>
            <input data-role="inline-category-name" placeholder="分类名称" />
          </label>
        ` : `
          <button class="new-category" data-action="new-category">
            <span>＋</span>
            新建分类
          </button>
        `}

        <div class="sidebar-footer">
          <button class="nav-item footer-nav ${isSettingsView ? "active" : ""}" data-view="settings">
            <span class="nav-icon">⚙</span>
            <span>设置</span>
            <b></b>
          </button>
        </div>
      </aside>

      <section class="workspace">
        <header class="toolbar">
          <div>
            <div class="eyebrow">${state.isTauri ? "本地软件管理" : "浏览器预览模式"}</div>
            <h1>${title}</h1>
          </div>

          <div class="toolbar-actions ${isSettingsView ? "hidden" : ""}">
            <label class="search">
              <span>⌕</span>
              <input value="${escapeHtml(state.query)}" placeholder="搜索软件、分类或备注" data-role="search" />
            </label>

            <div class="density-switch" aria-label="网格密度">
              <button class="${state.density === "comfortable" ? "active" : ""}" data-density="comfortable">舒适</button>
              <button class="${state.density === "compact" ? "active" : ""}" data-density="compact">紧凑</button>
            </div>

            <button class="primary-action" data-action="scan" ${state.loading ? "disabled" : ""}>${state.loading ? "扫描中" : "扫描"}</button>
          </div>
        </header>

        <section class="content">
          <div class="content-head">
            <div>
              <h2 data-role="content-title">${title}</h2>
              <p data-role="content-count">${isSettingsView ? "应用偏好和软件库管理" : `${visibleApps.length} 个应用 · ${state.isTauri ? "真实软件库" : "静态 UI 原型"}`}</p>
            </div>
            ${isSettingsView || isRemoteView ? "" : `<button class="ghost-button" data-action="delete-category" ${["favorites", "all"].includes(state.view) ? "disabled" : ""}>删除分类</button>`}
          </div>

          <div data-role="content-body">
            ${renderMainContent(visibleApps)}
          </div>
        </section>
      </section>
    </main>
    ${state.toast ? `<div class="toast">${escapeHtml(state.toast)}</div>` : ""}
    ${renderContextMenu()}
    ${renderModal()}
  `;

  bindEvents();
  renderedView = state.view;
  if (preserveContentScroll) {
    document.querySelector(".content")?.scrollTo({ top: previousContentScroll });
  }
  if (state.creatingCategory) {
    window.setTimeout(() => document.querySelector("[data-role='inline-category-name']")?.focus(), 0);
  }
  const duration = performance.now() - startedAt;
  const afterNodeCount = document.getElementsByTagName("*").length;
  if (duration >= SLOW_RENDER_THRESHOLD_MS || state.view === "settings") {
    debugLog(`render done id=${renderId} view=${state.view} duration=${duration.toFixed(1)}ms nodes=${beforeNodeCount}->${afterNodeCount} apps=${state.apps.length} remote=${state.remoteApps.length} review=${state.reviewApps.length} ${memoryDebugText()}`);
  }
}

function renderChromeState() {
  document.querySelector(".toast")?.remove();
  if (state.toast) {
    const toast = document.createElement("div");
    toast.className = "toast";
    toast.textContent = state.toast;
    document.body.appendChild(toast);
  }
}

function getRemoteNavCount() {
  const activeUploads = state.uploadQueue.filter((item) => getTransfer("upload", item.id)).length;
  return state.remoteApps.length + activeUploads;
}

function navItem(id, icon, label, count) {
  return `
    <button class="nav-item ${state.view === id ? "active" : ""}" data-view="${id}">
      <span class="nav-icon">${icon}</span>
      <span>${escapeHtml(label)}</span>
      <b>${count}</b>
    </button>
  `;
}

function renderGrid(items) {
  const sortableFavorites = state.view === "favorites" && !state.query.trim();
  const visibleItems = items.slice(0, APP_GRID_RENDER_LIMIT);
  const remaining = Math.max(0, items.length - visibleItems.length);
  return `
    <div class="app-grid ${state.density}">
      ${visibleItems.map((item) => `
        <article class="app-card ${state.dragFavoriteId === item.id ? "dragging" : ""} ${state.dragFavoriteTargetId === item.id ? "drag-over" : ""}" data-app="${item.id}" ${sortableFavorites ? `data-favorite-sort="true"` : ""}>
          ${renderAppIcon(item)}
          <h3>${escapeHtml(item.name)}</h3>
          <p>${escapeHtml(item.note)}</p>
          ${sortableFavorites ? `<span class="drag-handle" title="拖动调整常用软件顺序">↕</span>` : ""}
        </article>
      `).join("")}
    </div>
    ${remaining ? `<div class="remote-empty">还有 ${remaining} 个软件未显示，请使用搜索或分类缩小范围。</div>` : ""}
  `;
}

function renderMainContent(visibleApps) {
  if (state.view === "settings") return renderSettingsPage();
  if (state.view === "remote") return renderRemoteApps("menu");
  return visibleApps.length ? renderGrid(visibleApps) : renderEmpty();
}

function renderContent() {
  const visibleApps = getVisibleApps();
  const title = getTitle();
  const contentTitle = document.querySelector("[data-role='content-title']");
  const contentCount = document.querySelector("[data-role='content-count']");
  const contentBody = document.querySelector("[data-role='content-body']");

  if (contentTitle) contentTitle.textContent = title;
  if (contentCount) {
    contentCount.textContent = state.view === "settings"
      ? "应用偏好和软件库管理"
      : `${visibleApps.length} 个应用 · ${state.isTauri ? "真实软件库" : "静态 UI 原型"}`;
  }
  if (contentBody) {
    contentBody.innerHTML = renderMainContent(visibleApps);
  }

  bindAppCards();
  bindActionButtons(contentBody);
}

function renderServerStatusSummary() {
  const statusElement = document.querySelector(".server-status");
  if (!statusElement) return;
  const running = Boolean(state.serverStatus?.running);
  statusElement.classList.toggle("online", running);
  statusElement.innerHTML = `
    <span>${running ? "服务端运行中" : "服务端未运行"}</span>
    <strong>${running ? `${escapeHtml(state.serverStatus.host)}:${escapeHtml(state.serverStatus.port)}` : "切换到服务端模式并保存后启动"}</strong>
  `;
}

function renderThemeSelectionState() {
  document.querySelectorAll("[data-theme]").forEach((button) => {
    button.classList.toggle("active", button.dataset.theme === state.theme);
  });
  const themeRow = [...document.querySelectorAll(".setting-row")]
    .find((row) => row.querySelector("span")?.textContent === "主题");
  const value = themeRow?.querySelector("strong");
  if (value) value.textContent = themeLabel(state.theme);
}

function formatCheckedAt(value) {
  if (!value) return "未检测";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - Number(value));
  if (seconds < 5) return "刚刚";
  if (seconds < 60) return `${seconds} 秒前`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  return `${hours} 小时前`;
}

function renderSettingsPage() {
  return `
    <div class="settings-page">
      <section class="settings-panel">
        <div>
          <h3>软件库</h3>
          <p>软件库跟随 AppManager.exe 所在目录，用于存放分类文件夹、配置和图标缓存。</p>
        </div>
        <div class="settings-path" title="${escapeHtml(state.libraryPath)}">${escapeHtml(state.libraryPath)}</div>
        <button class="primary-action" data-action="open-library">打开软件库目录</button>
      </section>

      <section class="settings-panel">
        <div>
          <h3>下载缓存</h3>
          <p>服务端下载大软件时会缓存已打包的 zip，用空间换取下次下载速度。清空缓存不会删除软件库中的真实软件文件。</p>
        </div>
        <div class="cache-summary">
          <div>
            <span>缓存包</span>
            <strong>${Number(state.packageCache?.fileCount || 0)} 个</strong>
          </div>
          <div>
            <span>占用空间</span>
            <strong>${formatBytes(state.packageCache?.totalSize || 0)}</strong>
          </div>
        </div>
        <div class="settings-path" title="${escapeHtml(state.packageCache?.path || "")}">${escapeHtml(state.packageCache?.path || "AppManagerLibrary/config/package-cache")}</div>
        <div class="settings-action-row">
          <button class="ghost-button" data-action="refresh-package-cache">刷新缓存信息</button>
          <button class="danger-action" data-action="clear-package-cache" ${Number(state.packageCache?.fileCount || 0) ? "" : "disabled"}>清空下载缓存</button>
        </div>
      </section>

      <section class="settings-panel">
        <div>
          <h3>界面</h3>
          <p>主题和密度只影响当前界面显示，不改变软件库内容。</p>
        </div>
        <div class="setting-row">
          <span>主题</span>
          <strong>${themeLabel(state.theme)}</strong>
        </div>
        <div class="theme-grid">
          ${renderThemeOption("light", "浅色", "明亮清爽，适合日常管理")}
          ${renderThemeOption("dark", "深色", "降低夜间和暗光环境亮度")}
          ${renderThemeOption("green", "青绿", "更柔和的绿色工作台")}
        </div>
        <div class="setting-row">
          <span>网格密度</span>
          <strong>${state.density === "comfortable" ? "舒适" : "紧凑"}</strong>
        </div>
      </section>

      <section class="settings-panel">
        <div>
          <h3>运行模式</h3>
          <p>网络服务会按阶段实现。当前可以先保存模式选择，服务端和客户端能力后续逐步启用。</p>
        </div>
        <div class="mode-grid">
          ${renderModeOption("local", "本地模式", "只管理当前电脑的软件库")}
          ${renderModeOption("server", "服务端模式", "共享软件库，供客户端查看和下载")}
          ${renderModeOption("client", "客户端模式", "连接服务端并下载软件到本机")}
        </div>
      </section>

      <section class="settings-panel">
        <div>
          <h3>服务端设置</h3>
          <p>当前阶段提供只读 API：客户端可以查看服务端信息、分类和软件列表。下载能力会在下一阶段加入。</p>
        </div>
        <div class="server-status ${state.serverStatus?.running ? "online" : ""}">
          <span>${state.serverStatus?.running ? "服务端运行中" : "服务端未运行"}</span>
          <strong>${state.serverStatus?.running ? `${state.serverStatus.host}:${state.serverStatus.port}` : "切换到服务端模式并保存后启动"}</strong>
        </div>
        <div class="settings-form-grid">
          <label class="field compact-field">
            监听地址
            <input value="${escapeHtml(state.serverHost)}" data-role="server-host" />
          </label>
          <label class="field compact-field">
            监听端口
            <input type="number" min="1" max="65535" value="${escapeHtml(state.serverPort)}" data-role="server-port" />
          </label>
          <label class="field compact-field">
            用户名
            <input value="${escapeHtml(state.serverUsername)}" data-role="server-username" />
          </label>
          <label class="field compact-field">
            密码
            <input type="password" autocomplete="off" value="${escapeHtml(state.serverPassword)}" data-role="server-password" />
          </label>
        </div>
        <label class="toggle-row">
          <span>
            <strong>允许客户端下载</strong>
            <small>本阶段只保存该选项，实际下载 API 下一阶段实现</small>
          </span>
          <input type="checkbox" data-role="server-allow-downloads" ${state.serverAllowDownloads ? "checked" : ""} />
        </label>
        <button class="primary-action settings-save" data-action="save-settings">保存服务端设置</button>
      </section>

      <section class="settings-panel">
        <div>
          <h3>客户端设置</h3>
          <p>客户端可以连接另一台电脑上的 AppManager 服务端，查看软件列表并下载到本机软件库。</p>
        </div>
        <div class="settings-form-grid">
          <label class="field compact-field">
            服务端地址
            <input value="${escapeHtml(state.clientHost)}" data-role="client-host" />
          </label>
          <label class="field compact-field">
            服务端端口
            <input type="number" min="1" max="65535" value="${escapeHtml(state.clientPort)}" data-role="client-port" />
          </label>
          <label class="field compact-field">
            用户名
            <input value="${escapeHtml(state.clientUsername)}" data-role="client-username" />
          </label>
          <label class="field compact-field">
            密码
            <input type="password" autocomplete="off" value="${escapeHtml(state.clientPassword)}" data-role="client-password" />
          </label>
        </div>
        <div class="settings-action-row">
          <button class="primary-action" data-action="save-settings">保存客户端设置</button>
          <button class="ghost-button" data-action="test-client">测试连接</button>
          <button class="ghost-button" data-action="fetch-remote-apps">获取服务端软件列表</button>
        </div>
        ${renderRemoteSettingsSummary()}
      </section>

      <section class="settings-panel">
        <div>
          <h3>未审核软件</h3>
          <p>客户端上传的软件会先进入服务端的“未审核软件”目录，确认后再加入正式软件库。</p>
        </div>
        <div class="settings-action-row">
          <button class="ghost-button" data-action="refresh-review-apps">刷新未审核列表</button>
          <button class="ghost-button" data-action="open-review-folder">打开未审核目录</button>
        </div>
        ${renderReviewSettingsSummary()}
      </section>

      <section class="settings-panel">
        <div>
          <h3>启动</h3>
          <p>开机自启会写入当前 Windows 用户的启动项。关闭后会移除 AppManager 的启动项。</p>
        </div>
        <label class="toggle-row">
          <span>
            <strong>开机自启</strong>
            <small>登录 Windows 后自动启动 AppManager</small>
          </span>
          <input type="checkbox" data-role="autostart" data-action="save-settings" ${state.autostartEnabled ? "checked" : ""} />
        </label>
      </section>

      <section class="settings-panel">
        <div>
          <h3>关于</h3>
          <p>AppManager 是本地软件分类、扫描和启动工具。</p>
        </div>
        <div class="setting-row">
          <span>运行模式</span>
          <strong>${state.isTauri ? "桌面应用" : "浏览器预览"}</strong>
        </div>
        <div class="setting-row">
          <span>版本</span>
          <strong>0.1.0</strong>
        </div>
      </section>
    </div>
  `;
}

function renderModeOption(value, title, description) {
  return `
    <button class="mode-option ${state.runMode === value ? "active" : ""}" data-run-mode="${value}" data-action="set-run-mode">
      <strong>${title}</strong>
      <span>${description}</span>
    </button>
  `;
}

function renderThemeOption(value, title, description) {
  return `
    <button class="theme-option ${state.theme === value ? "active" : ""}" data-theme="${value}">
      <span class="theme-swatch ${value}"></span>
      <strong>${escapeHtml(title)}</strong>
      <small>${escapeHtml(description)}</small>
    </button>
  `;
}

function renderRemoteSettingsSummary() {
  if (isLocalMode()) {
    return `<div class="remote-empty">当前为本地模式，远程连接功能已关闭。</div>`;
  }
  if (isServerMode()) {
    return `<div class="remote-empty">当前为服务端模式，客户端连接功能未启用。</div>`;
  }
  return `
    <div class="remote-empty">
      已缓存 ${state.remoteApps.length} 个服务端软件。完整列表请在“远程连接”界面查看。
    </div>
  `;
}

function renderReviewSettingsSummary() {
  if (!isServerMode()) {
    return `<div class="remote-empty">当前不是服务端模式，未审核软件功能未启用。</div>`;
  }
  return `
    <div class="remote-empty">
      当前有 ${state.reviewApps.length} 个待审核上传。完整列表请在“远程连接”界面查看。
    </div>
  `;
}

function renderRemoteApps(source = "settings") {
  if (isLocalMode()) {
    const content = `<div class="remote-empty">当前为本地模式，远程连接功能已关闭。</div>`;
    return source === "menu"
      ? `
        <div class="remote-page">
          <div data-role="remote-status-panel">${renderRemoteStatusPanel()}</div>
          ${content}
        </div>
      `
      : content;
  }

  const uploadItems = state.uploadQueue.filter((item) => getTransfer("upload", item.id));
  const remoteItems = isClientMode() ? state.remoteApps : [];
  const visibleUploadItems = uploadItems.slice(0, REMOTE_LIST_RENDER_LIMIT);
  const visibleRemoteItems = remoteItems.slice(0, Math.max(0, REMOTE_LIST_RENDER_LIMIT - visibleUploadItems.length));
  const hiddenRemoteCount = Math.max(0, uploadItems.length + remoteItems.length - visibleUploadItems.length - visibleRemoteItems.length);
  const emptyText = isServerMode()
    ? "当前为服务端模式，客户端连接和服务端软件列表功能未启用。"
    : source === "menu"
      ? "还没有服务端软件列表。请先保存客户端设置，然后获取服务端软件列表。"
      : "还没有服务端软件列表。请先保存客户端设置，然后点击“获取服务端软件列表”。";
  const content = !remoteItems.length && !uploadItems.length
    ? `
      <div class="remote-empty">
        ${emptyText}
      </div>
    `
    : `
      <div class="remote-app-list">
        ${visibleUploadItems.map((item) => renderTransferAppRow(item, "upload")).join("")}
        ${visibleRemoteItems.map((item) => `
          ${renderTransferAppRow(item, "download")}
        `).join("")}
      </div>
      ${hiddenRemoteCount ? `<div class="remote-empty">还有 ${hiddenRemoteCount} 个远程条目未显示，请使用搜索缩小范围。</div>` : ""}
    `;
  const reviewContent = isServerMode()
    ? `
      <section class="connected-clients">
        <h3>未审核软件</h3>
        <div class="settings-action-row">
          <button class="ghost-button" data-action="refresh-review-apps">刷新未审核列表</button>
          <button class="ghost-button" data-action="open-review-folder">打开未审核目录</button>
        </div>
        ${renderReviewApps()}
      </section>
    `
    : "";

  if (source === "menu") {
    return `
      <div class="remote-page">
        <div data-role="remote-status-panel">${renderRemoteStatusPanel()}</div>
        ${isClientMode() ? `
          <div class="settings-action-row">
            <button class="ghost-button" data-action="refresh-connection-status">刷新连接状态</button>
            <button class="primary-action" data-action="fetch-remote-apps">获取服务端软件列表</button>
          </div>
        ` : ""}
        ${content}
        ${reviewContent}
      </div>
    `;
  }

  return content;
}

function renderRemoteStatusPanel() {
  const client = isClientMode() ? (state.clientStatus || {
    configured: false,
    online: false,
    host: state.clientHost,
    port: state.clientPort,
    username: state.clientUsername,
    message: "未检测"
  }) : inactiveClientStatus();
  const server = isServerMode() ? (state.serverStatus || inactiveServerStatus()) : inactiveServerStatus();
  const clients = server.clients || [];
  const onlineClientCount = clients.filter((item) => item.online).length;
  return `
    <section class="remote-status-grid">
      <article class="connection-card ${client.online ? "online" : ""}">
        <div>
          <span>客户端连接</span>
          <strong>${client.online ? "已连接" : client.configured ? "未连接" : "未配置"}</strong>
        </div>
        <p>${escapeHtml(client.message || "未检测")}</p>
        <dl>
          <dt>服务端</dt><dd>${escapeHtml(client.host || "-")}:${escapeHtml(client.port || "-")}</dd>
          <dt>用户</dt><dd>${escapeHtml(client.username || "-")}</dd>
          <dt>检测时间</dt><dd>${formatCheckedAt(client.checkedAt)}</dd>
          <dt>下载</dt><dd>${client.allowDownloads === true ? "允许" : client.allowDownloads === false ? "禁止" : "未知"}</dd>
        </dl>
      </article>
      <article class="connection-card ${server.running ? "online" : ""}">
        <div>
          <span>本机服务端</span>
          <strong>${server.running ? "运行中" : "未运行"}</strong>
        </div>
        <p>${server.running ? `只监听 ${escapeHtml(server.host)}:${escapeHtml(server.port)}；客户端源端口不会作为服务端口开放` : "切换到服务端模式并保存后启动"}</p>
        <dl>
          <dt>在线客户端</dt><dd>${onlineClientCount} / ${clients.length} 个</dd>
          <dt>监听端口</dt><dd>${server.running ? escapeHtml(server.port) : "-"}</dd>
        </dl>
      </article>
    </section>
    ${clients.length ? `
      <section class="connected-clients">
        <h3>最近连接客户端</h3>
        ${clients.map((client) => `
          <div class="client-row ${client.online ? "online" : "offline"}">
            <span>${escapeHtml(client.address)}</span>
            <strong>${escapeHtml(client.username || "anonymous")}</strong>
            <small>${client.online ? "在线" : "最近离线"} · ${escapeHtml(client.lastPath || "")}</small>
          </div>
        `).join("")}
      </section>
    ` : ""}
  `;
}

function renderRemoteStatusOnly() {
  const statusPanel = document.querySelector("[data-role='remote-status-panel']");
  if (statusPanel) {
    statusPanel.innerHTML = renderRemoteStatusPanel();
    bindActionButtons(statusPanel);
  } else {
    renderContent();
  }
}

function renderTransferAppRow(item, direction) {
  const transfer = getTransfer(direction, item.id);
  const active = isTransferActive(transfer);
  const isUpload = direction === "upload";
  return `
    <article class="remote-app" data-transfer-key="${direction}-${item.id}" data-remote-app-id="${direction === "download" ? item.id : ""}">
      ${renderAppIcon({
        iconDataUrl: item.iconDataUrl || "",
        accent: isUpload ? "teal" : "blue",
        initials: getInitials(item.name)
      })}
      <div class="remote-app-main">
        <h4>${escapeHtml(item.name)}</h4>
        <p>${escapeHtml(item.categoryName || (isUpload ? "\u5f85\u4e0a\u4f20" : "\u8fdc\u7a0b\u8f6f\u4ef6"))} \u00b7 ${escapeHtml(item.note || (isUpload ? "\u6b63\u5728\u4e0a\u4f20\u5230\u670d\u52a1\u7aef" : "\u670d\u52a1\u7aef\u8f6f\u4ef6"))}</p>
      </div>
      ${renderInlineTransfer(item, direction)}
      ${isUpload
        ? `<button class="primary-action" disabled>${active ? "\u4e0a\u4f20\u4e2d" : "\u5df2\u4e0a\u4f20"}</button>`
        : `<button class="primary-action" data-action="download-remote-app" data-app-id="${item.id}" ${active ? "disabled" : ""}>${active ? "\u4e0b\u8f7d\u4e2d" : "\u4e0b\u8f7d"}</button>`}
    </article>
  `;
}

function renderInlineTransfer(item, direction = "download") {
  const transfer = getTransfer(direction, item.id);
  if (!transfer) {
    return `<div class="inline-transfer idle"><span>${direction === "upload" ? "\u7b49\u5f85\u4e0a\u4f20" : "\u7b49\u5f85\u4e0b\u8f7d"}</span></div>`;
  }

  const parts = transferDisplayParts(transfer);

  return `
    <div class="inline-transfer">
      <div class="inline-transfer-row">
        <strong data-transfer-role="status">${parts.statusText}</strong>
        <span data-transfer-role="percent">${parts.percentText}</span>
      </div>
      <div class="inline-transfer-bar">
        <i data-transfer-role="bar" style="width:${parts.percent}%"></i>
      </div>
      <div class="inline-transfer-meta">
        <span data-transfer-role="size">${parts.sizeText}</span>
        <span data-transfer-role="speed">${parts.speedText}</span>
      </div>
    </div>
  `;
}

function transferDisplayParts(transfer) {
  const percent = Math.max(0, Math.min(100, Number(transfer.percent || 0)));
  const transferred = Number(transfer.transferred || 0);
  const total = Number(transfer.total || 0);
  const speed = Number(transfer.speed || 0);
  const sizeText = total > 0
    ? `${formatBytes(transferred)} / ${formatBytes(total)}`
    : transferred > 0
      ? `${formatBytes(transferred)} / \u8ba1\u7b97\u4e2d`
      : "\u7b49\u5f85\u6587\u4ef6\u5927\u5c0f";
  const speedText = transfer.status === "packing"
    ? "\u51c6\u5907\u4e2d"
    : transfer.status === "extracting"
      ? "\u6b63\u5728\u89e3\u538b"
      : transfer.status === "installing"
        ? "\u6b63\u5728\u5165\u5e93"
        : transfer.status === "done"
          ? "\u5b8c\u6210"
          : speed > 0
            ? `${formatBytes(speed)}/s`
            : "\u8fde\u63a5\u4e2d";

  return {
    percent,
    statusText: transferStatusText(transfer),
    percentText: `${percent.toFixed(1)}%`,
    sizeText,
    speedText
  };
}

function updateTransferElement(progress) {
  if (state.view !== "remote" && state.view !== "settings") return false;
  const key = `${progress.direction}-${progress.appId}`;
  const card = document.querySelector(`[data-transfer-key="${CSS.escape(key)}"]`);
  if (!card) return false;

  const transfer = getTransfer(progress.direction, progress.appId);
  if (!transfer) return false;
  const parts = transferDisplayParts(transfer);
  const status = card.querySelector("[data-transfer-role='status']");
  const percent = card.querySelector("[data-transfer-role='percent']");
  const bar = card.querySelector("[data-transfer-role='bar']");
  const size = card.querySelector("[data-transfer-role='size']");
  const speed = card.querySelector("[data-transfer-role='speed']");
  if (!status || !percent || !bar || !size || !speed) return false;

  status.textContent = parts.statusText;
  percent.textContent = parts.percentText;
  bar.style.width = `${parts.percent}%`;
  size.textContent = parts.sizeText;
  speed.textContent = parts.speedText;
  const button = card.querySelector("[data-action='download-remote-app']");
  if (button) {
    const active = isTransferActive(transfer);
    button.disabled = active;
    button.textContent = active ? "\u4e0b\u8f7d\u4e2d" : "\u4e0b\u8f7d";
  }
  return true;
}

function transferStatusText(transfer) {
  if (!transfer) return "";
  if (transfer.status === "packing") return "\u6b63\u5728\u6253\u5305";
  if (transfer.status === "running") return "\u6b63\u5728\u4f20\u8f93";
  if (transfer.status === "extracting") return "\u6b63\u5728\u89e3\u538b";
  if (transfer.status === "installing") return "\u6b63\u5728\u626b\u63cf\u5165\u5e93";
  if (transfer.status === "done") return "\u5df2\u5b8c\u6210";
  if (transfer.status === "error") return transfer.direction === "upload" ? "\u4e0a\u4f20\u5931\u8d25" : "\u4e0b\u8f7d\u5931\u8d25";
  return "\u51c6\u5907\u4e2d";
}

function renderReviewApps() {
  if (!state.reviewApps.length) {
    return `<div class="remote-empty">还没有待审核上传。客户端上传后会显示在这里。</div>`;
  }
  const visibleItems = state.reviewApps.slice(0, REVIEW_LIST_RENDER_LIMIT);
  const remaining = Math.max(0, state.reviewApps.length - visibleItems.length);

  return `
    <div class="remote-app-list">
      ${visibleItems.map((item) => `
        <article class="remote-app review-app">
          <div class="app-icon teal">${escapeHtml(getInitials(item.name))}</div>
          <div>
            <h4>${escapeHtml(item.name)}</h4>
            <p>${escapeHtml(item.categoryName)} · ${formatBytes(item.size)} · ${escapeHtml(item.fileName)}</p>
          </div>
          <div class="review-actions">
            <button class="primary-action" data-action="approve-review-app" data-review-id="${item.id}">通过</button>
            <button class="ghost-button" data-action="reject-review-app" data-review-id="${item.id}">拒绝</button>
          </div>
        </article>
      `).join("")}
    </div>
    ${remaining ? `<div class="remote-empty">还有 ${remaining} 个待审核软件未显示，请分批处理。</div>` : ""}
  `;
}

function getTransfer(direction, appId) {
  return state.transfers[`${direction}-${appId}`];
}

function isTransferActive(transfer) {
  return transfer && !["done", "error"].includes(transfer.status);
}

function renderContextMenu() {
  if (!state.contextMenu) return "";

  const item = state.apps.find((appItem) => appItem.id === state.contextMenu.appId);
  if (!item) return "";

  return `
    <div class="context-menu" style="left:${state.contextMenu.x}px; top:${state.contextMenu.y}px">
      <button data-action="launch" data-app-id="${item.id}">
        <span>▶</span>
        打开
      </button>
      <button data-action="launch-admin" data-app-id="${item.id}">
        <span>◇</span>
        以管理员身份运行
      </button>
      <button data-action="reveal" data-path="${escapeHtml(item.folderPath)}">
        <span>□</span>
        打开所在目录
      </button>
      <button data-action="favorite" data-app-id="${item.id}">
        <span>☆</span>
        ${item.favorite ? "取消收藏" : "收藏"}
      </button>
      <button data-action="move-app" data-app-id="${item.id}">
        <span>↪</span>
        移动到分类
      </button>
      <button data-action="upload-app" data-app-id="${item.id}">
        <span>⇧</span>
        上传到服务端
      </button>
      <button data-action="edit-app" data-app-id="${item.id}">
        <span>i</span>
        编辑信息
      </button>
      <button class="danger-menu-item" data-action="delete-app" data-app-id="${item.id}">
        <span>×</span>
        删除软件
      </button>
    </div>
  `;
}

function renderAppIcon(item) {
  if (item.iconDataUrl) {
    return `
      <div class="app-icon image-icon">
        <img src="${escapeHtml(item.iconDataUrl)}" alt="" />
      </div>
    `;
  }

  return `<div class="app-icon ${item.accent}">${escapeHtml(item.initials)}</div>`;
}

function renderEmpty() {
  return `
    <div class="empty-state">
      <div class="empty-art">⌕</div>
      <h2>${state.query ? "没有找到匹配的软件" : "这里还没有软件"}</h2>
      <p>${state.query ? "换个关键词试试，或清空搜索后浏览全部应用。" : "把软件文件夹放入这个分类，然后点击扫描。"}</p>
      <button class="primary-action" data-action="scan">扫描此分类</button>
    </div>
  `;
}

function renderModal() {
  if (state.modal === "new-category") {
    return `
      <div class="modal-backdrop">
        <section class="modal">
          <h2>新建分类</h2>
          <p>创建后会在 AppManagerLibrary / Apps 下生成对应文件夹。</p>
          <label class="field">
            分类名称
            <input placeholder="例如：效率工具" data-role="category-name" autofocus />
          </label>
          <div class="modal-actions">
            <button class="ghost-button" data-action="close-modal">取消</button>
            <button class="primary-action" data-action="confirm-category">创建</button>
          </div>
        </section>
      </div>
    `;
  }

  if (state.modal === "delete-category") {
    const category = state.categories.find((item) => item.id === state.view);
    return `
      <div class="modal-backdrop">
        <section class="modal danger-modal">
          <h2>删除分类</h2>
          <p>确定要删除“${escapeHtml(category?.name ?? "")}”吗？</p>
          <div class="path-preview">${escapeHtml(category?.path ?? "")}</div>
          <label class="check-row">
            <input type="checkbox" checked data-role="delete-files" />
            同时删除真实文件夹和其中的软件文件
          </label>
          <div class="modal-actions">
            <button class="ghost-button" data-action="close-modal">取消</button>
            <button class="danger-action" data-action="confirm-delete">删除</button>
          </div>
        </section>
      </div>
    `;
  }

  if (state.modal === "edit-app") {
    const item = state.apps.find((appItem) => appItem.id === state.selectedAppId);
    if (!item) return "";
    const candidates = getExecutableOptions(item);

    return `
      <div class="modal-backdrop">
        <section class="modal">
          <h2>编辑软件信息</h2>
          <p>可以修改显示名称、描述和自定义图标。图标路径支持 png、jpg、jpeg、gif、webp、ico、bmp。</p>
          <label class="field">
            软件名称
            <input value="${escapeHtml(item.name)}" data-role="edit-app-name" />
          </label>
          <label class="field">
            描述
            <input value="${escapeHtml(item.note)}" placeholder="例如：建模工具、常用启动器" data-role="edit-app-note" />
          </label>
          <label class="field">
            启动程序
            <select data-role="edit-app-executable" ${candidates.length ? "" : "disabled"}>
              ${candidates.length
                ? candidates.map((path) => `<option value="${escapeHtml(path)}" ${path === item.executablePath ? "selected" : ""}>${escapeHtml(shortPath(path))}</option>`).join("")
                : `<option value="">未发现可执行文件，请重新扫描</option>`}
            </select>
          </label>
          <label class="field">
            自定义图标路径
            <input placeholder="例如：D:\\Icons\\xcom.png，不填则保留当前图标" data-role="edit-app-icon" />
          </label>
          <div class="modal-actions">
            <button class="ghost-button" data-action="close-modal">取消</button>
            <button class="primary-action" data-action="confirm-edit-app">保存</button>
          </div>
        </section>
      </div>
    `;
  }

  if (state.modal === "delete-app") {
    const item = state.apps.find((appItem) => appItem.id === state.selectedAppId);
    if (!item) return "";

    return `
      <div class="modal-backdrop">
        <section class="modal danger-modal">
          <h2>删除软件</h2>
          <p>确定要删除“${escapeHtml(item.name)}”吗？</p>
          <div class="path-preview">${escapeHtml(item.executablePath || item.folderPath)}</div>
          <label class="check-row">
            <input type="checkbox" data-role="delete-app-files" />
            同时删除真实文件或软件文件夹
          </label>
          <div class="modal-actions">
            <button class="ghost-button" data-action="close-modal">取消</button>
            <button class="danger-action" data-action="confirm-delete-app">删除</button>
          </div>
        </section>
      </div>
    `;
  }

  if (state.modal === "move-app") {
    const item = state.apps.find((appItem) => appItem.id === state.selectedAppId);
    if (!item) return "";
    const targetCategories = state.categories.filter((category) => category.id !== item.categoryId);

    return `
      <div class="modal-backdrop">
        <section class="modal">
          <h2>移动到分类</h2>
          <p>选择目标分类后，AppManager 会移动真实软件文件或软件文件夹。</p>
          <label class="field">
            目标分类
            <select data-role="move-category-id" ${targetCategories.length ? "" : "disabled"}>
              ${targetCategories.map((category) => `<option value="${category.id}">${escapeHtml(category.name)}</option>`).join("")}
            </select>
          </label>
          ${targetCategories.length ? "" : `<div class="remote-empty">没有其他分类可以移动。</div>`}
          <div class="modal-actions">
            <button class="ghost-button" data-action="close-modal">取消</button>
            <button class="primary-action" data-action="confirm-move-app" ${targetCategories.length ? "" : "disabled"}>移动</button>
          </div>
        </section>
      </div>
    `;
  }

  return "";
}

function bindEvents() {
  document.querySelector(".sidebar")?.addEventListener("wheel", (event) => {
    const categoryList = document.querySelector(".category-list");
    if (!categoryList || categoryList.scrollHeight <= categoryList.clientHeight) return;

    categoryList.scrollTop += event.deltaY;
    event.preventDefault();
  }, { passive: false });

  document.querySelector(".shell")?.addEventListener("click", () => {
    if (state.contextMenu) {
      state.contextMenu = null;
      render();
    }
  });

  bindAppCards();

  document.querySelectorAll("[data-view]").forEach((button) => {
    button.addEventListener("click", () => {
      state.view = button.dataset.view;
      if (state.view === "remote") {
        render();
        window.setTimeout(() => refreshConnectionStatus({ silent: true }), 0);
        return;
      }
      render();
    });
  });

  document.querySelector("[data-role='search']")?.addEventListener("input", (event) => {
    state.query = event.target.value;
    renderContent();
  });

  document.querySelector("[data-role='inline-category-name']")?.addEventListener("keydown", async (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      await createCategory();
    }
    if (event.key === "Escape") {
      state.creatingCategory = false;
      render();
    }
  });

  document.querySelector("[data-role='inline-category-name']")?.addEventListener("blur", () => {
    const input = document.querySelector("[data-role='inline-category-name']");
    if (!input?.value.trim()) {
      state.creatingCategory = false;
      render();
    }
  });

  document.querySelectorAll("[data-density]").forEach((button) => {
    button.addEventListener("click", () => {
      state.density = button.dataset.density;
      render();
    });
  });

  document.querySelectorAll("[data-theme]").forEach((button) => {
    button.addEventListener("click", () => {
      const nextTheme = normalizeTheme(button.dataset.theme);
      if (nextTheme === state.theme) return;
      state.theme = nextTheme;
      applyTheme();
      renderThemeSelectionState();
      debugLog(`theme changed value=${state.theme} view=${state.view} ${memoryDebugText()}`);
    });
  });

  bindActionButtons(document);
}

function bindAppCards() {
  const finishFavoritePointerDrag = () => {
    state.dragFavoriteId = null;
    state.dragFavoriteTargetId = null;
    document.querySelectorAll(".app-card.dragging, .app-card.drag-over").forEach((item) => item.classList.remove("dragging", "drag-over"));
  };

  const updateFavoriteDragTarget = (event) => {
    if (!state.dragFavoriteId) return null;
    const element = document.elementFromPoint(event.clientX, event.clientY);
    const targetCard = element?.closest?.(".app-card[data-favorite-sort='true']");
    const targetId = targetCard?.dataset.app || null;
    document.querySelectorAll(".app-card.drag-over").forEach((item) => item.classList.remove("drag-over"));
    if (targetId && targetId !== state.dragFavoriteId) {
      state.dragFavoriteTargetId = targetId;
      targetCard.classList.add("drag-over");
      return targetId;
    }
    state.dragFavoriteTargetId = null;
    return null;
  };

  document.querySelectorAll(".app-card").forEach((card) => {
    card.addEventListener("click", async () => {
      if (state.contextMenu) return;
      if (Date.now() < state.suppressLaunchUntil) return;
      await launchApp(card.dataset.app);
    });

    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      state.contextMenu = {
        appId: card.dataset.app,
        x: Math.min(event.clientX, window.innerWidth - 190),
        y: Math.min(event.clientY, window.innerHeight - 150)
      };
      render();
    });

    if (card.dataset.favoriteSort === "true") {
      card.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        const sourceId = card.dataset.app;
        const startX = event.clientX;
        const startY = event.clientY;
        let dragging = false;

        const handlePointerMove = (moveEvent) => {
          const distance = Math.hypot(moveEvent.clientX - startX, moveEvent.clientY - startY);
          if (!dragging && distance < 6) return;
          if (!dragging) {
            dragging = true;
            state.dragFavoriteId = sourceId;
            state.dragFavoriteTargetId = null;
            state.suppressLaunchUntil = Date.now() + 1200;
            card.classList.add("dragging");
          }
          moveEvent.preventDefault();
          updateFavoriteDragTarget(moveEvent);
        };

        const handlePointerUp = async (upEvent) => {
          window.removeEventListener("pointermove", handlePointerMove);
          window.removeEventListener("pointerup", handlePointerUp);
          window.removeEventListener("pointercancel", handlePointerCancel);
          if (!dragging) return;

          const targetId = updateFavoriteDragTarget(upEvent) || state.dragFavoriteTargetId;
          finishFavoritePointerDrag();
          if (sourceId && targetId && sourceId !== targetId) {
            await reorderFavoriteApps(sourceId, targetId);
          }
        };

        const handlePointerCancel = () => {
          window.removeEventListener("pointermove", handlePointerMove);
          window.removeEventListener("pointerup", handlePointerUp);
          window.removeEventListener("pointercancel", handlePointerCancel);
          finishFavoritePointerDrag();
        };

        window.addEventListener("pointermove", handlePointerMove, { passive: false });
        window.addEventListener("pointerup", handlePointerUp);
        window.addEventListener("pointercancel", handlePointerCancel);
      });

      card.addEventListener("pointercancel", finishFavoritePointerDrag);

      card.addEventListener("dragstart", (event) => {
        state.dragFavoriteId = card.dataset.app;
        state.suppressLaunchUntil = Date.now() + 1200;
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", card.dataset.app);
        card.classList.add("dragging");
      });

      card.addEventListener("dragover", (event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
        card.classList.add("drag-over");
      });

      card.addEventListener("dragleave", () => {
        card.classList.remove("drag-over");
      });

      card.addEventListener("drop", async (event) => {
        event.preventDefault();
        card.classList.remove("drag-over");
        const sourceId = event.dataTransfer.getData("text/plain") || state.dragFavoriteId;
        const targetId = card.dataset.app;
        state.dragFavoriteId = null;
        if (sourceId && targetId && sourceId !== targetId) {
          await reorderFavoriteApps(sourceId, targetId);
        }
      });

      card.addEventListener("dragend", () => {
        state.dragFavoriteId = null;
        card.classList.remove("dragging", "drag-over");
      });
    }
  });
}

function bindActionButtons(root) {
  root?.querySelectorAll("[data-action]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      state.contextMenu = null;
      await handleAction(button);
    });
  });
}

async function handleAction(button) {
  const action = button.dataset.action;

  if (action === "new-category") {
    state.creatingCategory = true;
    render();
    return;
  }
  if (action === "delete-category") state.modal = "delete-category";
  if (action === "close-modal") {
    state.modal = null;
    state.selectedAppId = null;
  }

  if (action === "edit-app") {
    state.selectedAppId = button.dataset.appId;
    state.modal = "edit-app";
  }

  if (action === "delete-app") {
    state.selectedAppId = button.dataset.appId;
    state.modal = "delete-app";
  }

  if (action === "move-app") {
    state.selectedAppId = button.dataset.appId;
    state.modal = "move-app";
  }

  if (action === "confirm-category") {
    await createCategory();
    return;
  }

  if (action === "confirm-delete") {
    await deleteCategory();
    return;
  }

  if (action === "confirm-edit-app") {
    await updateAppInfo();
    return;
  }

  if (action === "confirm-delete-app") {
    await deleteApp();
    return;
  }

  if (action === "confirm-move-app") {
    await moveAppToCategory();
    return;
  }

  if (action === "set-run-mode") {
    state.runMode = button.dataset.runMode;
    state.clientStatus = isClientMode() ? idleClientStatus(state.clientStatus?.message || "未检测") : inactiveClientStatus();
    if (!isServerMode()) state.serverStatus = inactiveServerStatus();
    if (!isClientMode()) state.remoteApps = [];
    render();
    return;
  }

  if (action === "save-settings") {
    await saveSettings(button);
    return;
  }

  if (action === "test-client") {
    if (!isClientMode()) {
      showToast("当前不是客户端模式，客户端连接功能未启用");
      return;
    }
    if (await saveSettings(button, { silent: true })) await testClientConnection();
    return;
  }

  if (action === "fetch-remote-apps") {
    if (!isClientMode()) {
      showToast("当前不是客户端模式，不能获取服务端软件列表");
      return;
    }
    if (await saveSettings(button, { silent: true })) await fetchRemoteApps();
    return;
  }

  if (action === "refresh-connection-status") {
    await refreshConnectionStatus();
    return;
  }

  if (action === "refresh-package-cache") {
    await refreshPackageCache();
    return;
  }

  if (action === "clear-package-cache") {
    await clearPackageCache();
    return;
  }

  if (action === "download-remote-app") {
    if (!isClientMode()) {
      showToast("当前不是客户端模式，不能下载远程软件");
      return;
    }
    await downloadRemoteApp(button.dataset.appId);
    return;
  }

  if (action === "upload-app") {
    if (!isClientMode()) {
      showToast("当前不是客户端模式，不能上传到服务端");
      return;
    }
    await uploadAppToServer(button.dataset.appId);
    return;
  }

  if (action === "refresh-review-apps") {
    if (!isServerMode()) {
      showToast("当前不是服务端模式，未审核软件功能未启用");
      return;
    }
    await refreshReviewApps();
    return;
  }

  if (action === "approve-review-app") {
    await approveReviewApp(button.dataset.reviewId);
    return;
  }

  if (action === "reject-review-app") {
    await rejectReviewApp(button.dataset.reviewId);
    return;
  }

  if (action === "open-review-folder") {
    await reveal(`${state.libraryPath}\\Apps\\未审核软件`);
    return;
  }

  if (action === "scan") {
    await scan();
    return;
  }

  if (action === "favorite") {
    await toggleFavorite(button.dataset.appId);
    return;
  }

  if (action === "launch") {
    await launchApp(button.dataset.appId);
    return;
  }

  if (action === "launch-admin") {
    await launchApp(button.dataset.appId, true);
    return;
  }

  if (action === "reveal") {
    await reveal(button.dataset.path);
    return;
  }

  if (action === "open-library") {
    await reveal(state.libraryPath);
    return;
  }

  render();
}

async function createCategory() {
  const input = document.querySelector("[data-role='inline-category-name']") || document.querySelector("[data-role='category-name']");
  const name = input?.value ?? "";

  if (!state.isTauri) {
    const cleanName = name.trim();
    if (!cleanName) {
      showToast("请输入分类名称");
      input?.focus();
      return;
    }
    state.categories.push({ id: `demo-${Date.now()}`, name: cleanName, count: 0, path: `AppManager/Apps/${cleanName}` });
    state.creatingCategory = false;
    state.modal = null;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("create_category", { name });
    applyData(data);
    state.creatingCategory = false;
    state.modal = null;
    showToast("分类已创建");
  }, "创建分类失败");
}

async function deleteCategory() {
  const categoryId = state.view;
  const deleteFiles = Boolean(document.querySelector("[data-role='delete-files']")?.checked);

  if (!state.isTauri) {
    state.categories = state.categories.filter((item) => item.id !== categoryId);
    state.apps = state.apps.filter((item) => item.categoryId !== categoryId);
    state.view = "all";
    state.modal = null;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("delete_category", { categoryId, deleteFiles });
    applyData(data);
    state.view = "all";
    state.modal = null;
    showToast("分类已删除");
  }, "删除分类失败");
}

async function scan() {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会扫描真实文件");
    return;
  }

  await runTask(async () => {
    state.suppressLaunchUntil = Date.now() + 1200;
    const result = ["favorites", "all"].includes(state.view)
      ? await invoke("scan_all")
      : await invoke("scan_category", { categoryId: state.view });

    applyData(result.data);
    showToast(`扫描完成：新增 ${result.added} 个，更新 ${result.updated} 个，需处理 ${result.issues.length} 个`);
  }, "扫描失败");
}

async function toggleFavorite(appId) {
  if (!state.isTauri) {
    const item = state.apps.find((appItem) => appItem.id === appId);
    if (item) item.favorite = !item.favorite;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("toggle_favorite", { appId });
    applyData(data);
  }, "收藏操作失败");
}

async function reorderFavoriteApps(sourceId, targetId) {
  const favorites = sortFavorites(state.apps.filter((item) => item.favorite)).map((item) => item.id);
  const fromIndex = favorites.indexOf(sourceId);
  const toIndex = favorites.indexOf(targetId);
  if (fromIndex < 0 || toIndex < 0) return;

  favorites.splice(fromIndex, 1);
  favorites.splice(toIndex, 0, sourceId);
  state.favoriteOrder = favorites;
  state.suppressLaunchUntil = Date.now() + 800;
  render();

  if (!state.isTauri) return;

  await runTask(async () => {
    const data = await invoke("update_favorite_order", { appIds: favorites });
    const currentDensity = state.density;
    const currentTheme = state.theme;
    applyData(data);
    state.density = currentDensity;
    state.theme = currentTheme;
    applyTheme();
    showToast("常用软件顺序已保存");
  }, "保存常用软件顺序失败");
}

async function updateAppInfo() {
  state.suppressLaunchUntil = Date.now() + 1200;
  const appId = state.selectedAppId;
  const name = document.querySelector("[data-role='edit-app-name']")?.value ?? "";
  const note = document.querySelector("[data-role='edit-app-note']")?.value ?? "";
  const iconPath = document.querySelector("[data-role='edit-app-icon']")?.value ?? "";
  const executablePath = document.querySelector("[data-role='edit-app-executable']")?.value ?? null;

  if (!name.trim()) {
    showToast("请输入软件名称");
    return;
  }

  if (!state.isTauri) {
    const item = state.apps.find((appItem) => appItem.id === appId);
    if (item) {
      item.name = name.trim();
      item.note = note.trim();
      item.initials = getInitials(item.name);
    }
    state.modal = null;
    state.selectedAppId = null;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("update_app_info", {
      request: {
        appId,
        name,
        note,
        iconPath,
        executablePath
      }
    });
    applyData(data);
    state.modal = null;
    state.selectedAppId = null;
    showToast("软件信息已保存");
  }, "保存软件信息失败");
}

async function refreshServerUploadData() {
  if (!state.isTauri || state.runMode !== "server") return;
  try {
    const result = await invoke("init_library");
    applyData(result.data);
    state.reviewApps = await invoke("list_review_apps");
    renderContent();
  } catch (error) {
    debugLog(`server upload refresh failed error=${error}`);
  }
}

async function deleteApp() {
  const appId = state.selectedAppId;
  const deleteFiles = Boolean(document.querySelector("[data-role='delete-app-files']")?.checked);

  if (!state.isTauri) {
    state.apps = state.apps.filter((item) => item.id !== appId);
    state.modal = null;
    state.selectedAppId = null;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("delete_app", { appId, deleteFiles });
    applyData(data);
    state.modal = null;
    state.selectedAppId = null;
    showToast(deleteFiles ? "软件和文件已删除" : "软件已从列表删除");
  }, "删除软件失败");
}

async function moveAppToCategory() {
  const appId = state.selectedAppId;
  const categoryId = document.querySelector("[data-role='move-category-id']")?.value;
  if (!categoryId) {
    showToast("请选择目标分类");
    return;
  }

  if (!state.isTauri) {
    const item = state.apps.find((appItem) => appItem.id === appId);
    const category = state.categories.find((categoryItem) => categoryItem.id === categoryId);
    if (item && category) {
      item.categoryId = category.id;
      item.categoryName = category.name;
    }
    state.modal = null;
    state.selectedAppId = null;
    render();
    return;
  }

  await runTask(async () => {
    const data = await invoke("move_app_to_category", { appId, categoryId });
    applyData(data);
    state.modal = null;
    state.selectedAppId = null;
    showToast("软件已移动到目标分类");
  }, "移动软件失败");
}

async function saveSettings(source, options = {}) {
  const runMode = source.dataset.runMode || state.runMode;
  const autostartInput = document.querySelector("[data-role='autostart']");
  const serverPort = Number(document.querySelector("[data-role='server-port']")?.value || state.serverPort);
  const clientPort = Number(document.querySelector("[data-role='client-port']")?.value || state.clientPort);
  const settingsPayload = {
    runMode,
    theme: state.theme,
    gridDensity: state.density,
    autostartEnabled: source.dataset.role === "autostart"
      ? Boolean(source.checked)
      : Boolean(autostartInput?.checked),
    serverHost: document.querySelector("[data-role='server-host']")?.value || state.serverHost,
    serverPort,
    serverUsername: document.querySelector("[data-role='server-username']")?.value || state.serverUsername,
    serverPassword: document.querySelector("[data-role='server-password']")?.value ?? state.serverPassword,
    serverAllowDownloads: Boolean(document.querySelector("[data-role='server-allow-downloads']")?.checked),
    clientHost: document.querySelector("[data-role='client-host']")?.value || state.clientHost,
    clientPort,
    clientUsername: document.querySelector("[data-role='client-username']")?.value || state.clientUsername,
    clientPassword: document.querySelector("[data-role='client-password']")?.value ?? state.clientPassword
  };
  if (!Number.isInteger(serverPort) || serverPort < 1 || serverPort > 65535) {
    showToast("服务端端口必须在 1 到 65535 之间");
    return false;
  }

  if (!Number.isInteger(clientPort) || clientPort < 1 || clientPort > 65535) {
    showToast("客户端端口必须在 1 到 65535 之间");
    return false;
  }

  if (!state.isTauri) {
    state.runMode = settingsPayload.runMode;
    state.theme = normalizeTheme(settingsPayload.theme);
    applyTheme();
    state.autostartEnabled = settingsPayload.autostartEnabled;
    state.serverHost = settingsPayload.serverHost;
    state.serverPort = serverPort;
    state.serverUsername = settingsPayload.serverUsername;
    state.serverPassword = settingsPayload.serverPassword;
    state.serverAllowDownloads = settingsPayload.serverAllowDownloads;
    state.clientHost = settingsPayload.clientHost;
    state.clientPort = clientPort;
    state.clientUsername = settingsPayload.clientUsername;
    state.clientPassword = settingsPayload.clientPassword;
    state.clientStatus = isClientMode() ? idleClientStatus("设置已保存，尚未检测连接") : inactiveClientStatus();
    state.serverStatus = isServerMode() ? state.serverStatus : inactiveServerStatus();
    if (!isClientMode()) state.remoteApps = [];
    render();
    return true;
  }

  return await runTask(async () => {
    const data = await invoke("update_settings", {
      request: settingsPayload
    });
    applyData(data);
    state.serverStatus = isServerMode() ? await invoke("get_server_status") : inactiveServerStatus();
    state.clientStatus = isClientMode() ? idleClientStatus("设置已保存，尚未检测连接") : inactiveClientStatus();
    if (!isClientMode()) state.remoteApps = [];
    if (!options.silent) showToast("设置已保存");
  }, "保存设置失败");
}

async function testClientConnection() {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会连接服务端");
    return;
  }

  await runTask(async () => {
    const message = await invoke("test_client_connection");
    state.clientStatus = await invoke("get_client_connection_status");
    state.serverStatus = inactiveServerStatus();
    showToast(message);
  }, "测试连接失败");
}

async function fetchRemoteApps() {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会获取服务端软件");
    return;
  }

  await runTask(async () => {
    state.remoteApps = await invoke("fetch_remote_apps");
    state.clientStatus = await invoke("get_client_connection_status");
    state.serverStatus = inactiveServerStatus();
    state.view = "remote";
    showToast(`已获取 ${state.remoteApps.length} 个服务端软件`);
  }, "获取服务端软件失败");
}

async function refreshConnectionStatus(options = {}) {
  if (isLocalMode()) {
    state.serverStatus = inactiveServerStatus();
    state.clientStatus = inactiveClientStatus();
    if (state.view === "remote") renderRemoteStatusOnly();
    if (!options.silent) showToast("当前为本地模式，远程连接功能已关闭");
    return;
  }

  if (options.silent && state.isTauri) {
    if (connectionStatusRefreshing) return;
    connectionStatusRefreshing = true;
    try {
      state.serverStatus = isServerMode() ? await invoke("get_server_status") : inactiveServerStatus();
      state.clientStatus = isClientMode() ? await invoke("get_client_connection_status") : inactiveClientStatus();
      if (state.view === "remote") {
        renderRemoteStatusOnly();
      } else {
        renderServerStatusSummary();
      }
    } catch (error) {
      state.clientStatus = {
        configured: Boolean(state.clientHost && state.clientUsername && state.clientPassword),
        online: false,
        host: state.clientHost,
        port: state.clientPort,
        username: state.clientUsername,
        message: String(error),
        checkedAt: Math.floor(Date.now() / 1000)
      };
      if (state.view === "remote") renderRemoteStatusOnly();
    } finally {
      connectionStatusRefreshing = false;
    }
    return;
  }

  if (!state.isTauri) {
    if (!options.silent) showToast("浏览器预览模式不会检测连接状态");
    return;
  }

  await runTask(async () => {
    state.serverStatus = isServerMode() ? await invoke("get_server_status") : inactiveServerStatus();
    state.clientStatus = isClientMode() ? await invoke("get_client_connection_status") : inactiveClientStatus();
    if (!options.silent) {
      const message = isClientMode()
        ? (state.clientStatus?.online ? "远程连接正常" : `远程未连接：${state.clientStatus?.message || "未知状态"}`)
        : "服务端状态已刷新";
      showToast(message);
    }
  }, "刷新连接状态失败");
}

function startConnectionStatusPolling() {
  if (!state.isTauri || connectionStatusTimer) return;
  connectionStatusTimer = window.setInterval(() => {
    if (isLocalMode()) return;
    refreshConnectionStatus({ silent: true });
  }, CONNECTION_STATUS_INTERVAL_MS);
}

async function refreshPackageCache(options = {}) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会读取下载缓存");
    return;
  }

  await runTask(async () => {
    state.packageCache = await invoke("get_package_cache_info");
    if (!options.silent) {
      showToast(`下载缓存：${state.packageCache.fileCount} 个，${formatBytes(state.packageCache.totalSize)}`);
    }
  }, "读取下载缓存失败");
}

async function clearPackageCache() {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会清理下载缓存");
    return;
  }

  if (!window.confirm("确定要清空下载缓存吗？这不会删除软件库中的真实软件文件，但下次下载大软件时需要重新打包。")) {
    return;
  }

  await runTask(async () => {
    state.packageCache = await invoke("clear_package_cache");
    showToast("下载缓存已清空");
  }, "清空下载缓存失败");
}

async function downloadRemoteApp(appId) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会下载服务端软件");
    return;
  }

  const app = state.remoteApps.find((item) => item.id === appId);
  const key = `download-${appId}`;
  state.transfers[key] = {
    appId,
    appName: app?.name || "远程软件",
    direction: "download",
    transferred: 0,
    total: 0,
    speed: 0,
    percent: 0,
    status: "packing"
  };
  render();
  startTransferPolling("download", appId);

  try {
    const data = await invoke("download_remote_app", { appId, appName: app?.name || "远程软件" });
    applyData(data);
    state.transfers[key] = {
      ...state.transfers[key],
      transferred: state.transfers[key]?.total || state.transfers[key]?.transferred || 0,
      percent: 100,
      speed: 0,
      status: "done"
    };
    showToast("软件下载完成，已加入本机软件库");
  } catch (error) {
    state.transfers[key] = {
      ...state.transfers[key],
      status: "error"
    };
    showToast(`下载软件失败：${error}`);
  } finally {
    stopTransferPolling(key);
    render();
  }
}

async function uploadAppToServer(appId) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会上传软件");
    return;
  }

  const app = state.apps.find((item) => item.id === appId);
  const key = `upload-${appId}`;
  const queueItem = {
    id: appId,
    name: app?.name || "\u672c\u5730\u8f6f\u4ef6",
    categoryName: app?.categoryName || "\u672c\u5730\u8f6f\u4ef6",
    note: "\u6b63\u5728\u4e0a\u4f20\u5230\u670d\u52a1\u7aef",
    iconDataUrl: app?.iconDataUrl || ""
  };
  state.uploadQueue = [
    queueItem,
    ...state.uploadQueue.filter((item) => item.id !== appId)
  ];
  state.transfers[key] = {
    appId,
    appName: queueItem.name,
    direction: "upload",
    transferred: 0,
    total: 0,
    speed: 0,
    percent: 0,
    status: "packing"
  };
  state.view = "remote";
  render();
  startTransferPolling("upload", appId);

  try {
    const message = await invoke("upload_app_to_server", { appId, appName: queueItem.name });
    state.transfers[key] = {
      ...state.transfers[key],
      transferred: state.transfers[key]?.total || state.transfers[key]?.transferred || 0,
      percent: 100,
      speed: 0,
      status: "done"
    };
    showToast(message);
  } catch (error) {
    state.transfers[key] = {
      ...state.transfers[key],
      status: "error",
      speed: 0
    };
    showToast(`上传软件失败：${error}`);
  } finally {
    stopTransferPolling(key);
    render();
  }
}

async function refreshReviewApps(options = {}) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会读取未审核软件");
    return;
  }

  await runTask(async () => {
    state.reviewApps = await invoke("list_review_apps");
    if (!options.silent) showToast(`待审核软件 ${state.reviewApps.length} 个`);
  }, "读取未审核软件失败");
}

async function approveReviewApp(reviewId) {
  if (!state.isTauri) return;

  await runTask(async () => {
    const data = await invoke("approve_review_app", { reviewId });
    applyData(data);
    state.reviewApps = await invoke("list_review_apps");
    showToast("已加入软件库");
  }, "审核软件失败");
}

async function rejectReviewApp(reviewId) {
  if (!state.isTauri) return;

  await runTask(async () => {
    state.reviewApps = await invoke("reject_review_app", { reviewId });
    showToast("已拒绝并删除上传文件");
  }, "拒绝软件失败");
}

async function launchApp(appId, asAdmin = false) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会启动真实软件");
    return;
  }

  await runTask(async () => {
    const result = await invoke(asAdmin ? "launch_app_as_admin" : "launch_app", { appId });
    const app = state.apps.find((item) => item.id === result.appId);
    if (app) {
      app.launchCount = result.launchCount;
      app.lastLaunchedAt = result.lastLaunchedAt;
    }
    showToast(asAdmin ? "正在请求管理员权限启动" : "已发送启动命令");
  }, asAdmin ? "管理员权限启动失败" : "启动失败");
}

async function reveal(path) {
  if (!state.isTauri) {
    showToast("浏览器预览模式不会打开目录");
    return;
  }

  await runTask(async () => {
    await invoke("reveal_path", { path });
  }, "打开目录失败");
}

async function runTask(task, errorPrefix) {
  state.loading = true;
  renderLoadingState();
  try {
    await task();
    return true;
  } catch (error) {
    showToast(`${errorPrefix}：${error}`);
    return false;
  } finally {
    state.loading = false;
    renderLoadingState();
  }
}

function renderLoadingState() {
  document.querySelectorAll("[data-action='scan']").forEach((button) => {
    button.disabled = state.loading;
    button.textContent = state.loading ? "扫描中" : "扫描";
  });
}

function showToast(message) {
  state.toast = message;
  renderChromeState();
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    state.toast = "";
    renderChromeState();
  }, 3200);
}

function getInitials(value) {
  const words = value
    .replace(/[_-]+/g, " ")
    .split(" ")
    .filter(Boolean);

  if (words.length >= 2) {
    return `${words[0][0]}${words[1][0]}`.toUpperCase();
  }

  return value.slice(0, 2).toUpperCase();
}

function getExecutableOptions(item) {
  const values = [...(item.executableCandidates || [])];
  if (item.executablePath && !values.includes(item.executablePath)) {
    values.unshift(item.executablePath);
  }
  return values.filter(Boolean);
}

function shortPath(path) {
  if (!path) return "";
  const parts = path.replaceAll("\\", "/").split("/");
  return parts.length > 3 ? `.../${parts.slice(-3).join("/")}` : path;
}

function formatBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = bytes / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  return `${size >= 10 ? size.toFixed(1) : size.toFixed(2)} ${units[index]}`;
}

function escapeHtml(value) {
  return String(value ?? "").replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#039;"
  })[char]);
}

boot();
