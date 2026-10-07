// Workspace Hub 儀表板：services.json 驅動的卡片（一般服務／狀態卡／資訊卡／封存卡）＋個人待辦。
// 樣板（{file:}/{date:}）一律在 Rust 端展開，前端看不到 token 內容。
// 群組可收合（狀態存 localStorage）；封存／取消封存由 Rust 端改寫 services.json（只切 archived）。
(() => {
  "use strict";
  const invoke = (name, args) => window.__TAURI__?.core?.invoke
    ? window.__TAURI__.core.invoke(name, args)
    : Promise.reject(new Error("桌面連接尚未就緒，請從桌面捷徑開啟。"));
  const esc = v => String(v ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const $ = s => document.querySelector(s);
  const setErr = msg => { $("#err").textContent = msg; };

  const PROBE_INTERVAL_MS = 15000;      // 一般服務健康探測
  const STATUS_INTERVAL_MS = 60000;     // 狀態卡（schtasks／檔案）較重，放慢
  const COLLAPSED_KEY = "hub.collapsedGroups";   // localStorage：收合的群組名稱陣列（WebView2 使用者資料夾，重開 app 仍在）

  let cfg = null;
  let showArchived = false;
  const busy = new Set();      // 啟停過渡中的服務 id
  const health = {};           // id -> bool
  const statusInfo = {};       // id -> 狀態卡資料
  let confirming = null;       // 待確認的封存操作 { group, id|null, archived, name }
  let archiving = false;       // 封存改檔進行中

  // ---------- 收合狀態 ----------
  const collapsed = new Set((() => {
    try { const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) || "[]"); return Array.isArray(v) ? v.filter(x => typeof x === "string") : []; }
    catch { return []; }
  })());
  function toggleCollapsed(name) {
    collapsed.has(name) ? collapsed.delete(name) : collapsed.add(name);
    try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsed])); } catch { /* 無法持久化時仍可操作 */ }
    render();
  }

  // ---------- 設定視圖 ----------
  const isArchived = (g, s) => !!(g.archived || s.archived);
  function visibleGroups() {
    return (cfg?.groups || [])
      .map(g => ({ ...g, services: (g.services || []).filter(s => showArchived || !isArchived(g, s)) }))
      .filter(g => g.services.length);
  }
  function liveServices() {
    return (cfg?.groups || []).flatMap(g => (g.services || []).filter(s => !isArchived(g, s)).map(s => ({ ...s, _group: g })));
  }
  const findService = id => liveServices().find(s => s.id === id);

  // ---------- 卡片渲染 ----------
  const RESULT_LABEL = { success: "成功", running: "執行中", never: "尚未執行", failure: "失敗" };

  /// 封存／取消封存按鈕，或（確認中）同位置的 inline 確認列。群組已封存時服務卡不給個別按鈕（由群組層級處理）。
  function archiveControl(g, s) {
    if (s ? g.archived : false) return "";
    const key = { group: g.name, id: s ? s.id : null };
    const name = s ? s.name : g.name;
    const toArchive = s ? !s.archived : !g.archived;
    const label = toArchive ? (s ? "封存" : "封存群組") : "取消封存";
    const attr = `data-arch-group="${esc(key.group)}"${s ? ` data-arch-id="${esc(s.id)}"` : ""}`;
    if (confirming && confirming.group === key.group && confirming.id === key.id) {
      return `<span class="confirm" role="group" aria-label="確認">${toArchive ? "封存" : "取消封存"}「${esc(name)}」？
        <button type="button" class="yes" data-confirm="yes" ${archiving ? "disabled" : ""}>${archiving ? "…" : "確定"}</button>
        <button type="button" data-confirm="no" ${archiving ? "disabled" : ""}>取消</button></span>`;
    }
    return `<button type="button" class="arch" ${attr} data-arch-to="${toArchive ? "1" : "0"}" ${archiving ? "disabled" : ""} title="${toArchive ? "封存（改寫 services.json）" : "取消封存"}">${label}</button>`;
  }

  function renderArchived(g, s) {
    return `<div class="card archived"><div class="dot none"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div><div class="ds">${esc(s.desc ?? "")}</div></div>${archiveControl(g, s)}</div>`;
  }
  function renderInfo(g, s) {
    return `<div class="card info"><div class="dot none"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div><div class="ds">${esc(s.desc ?? "")}</div>
      ${s.note ? `<div class="lastline">${esc(s.note)}</div>` : ""}</div>${archiveControl(g, s)}</div>`;
  }

  /// 狀態卡的燈號與文字：排程（taskName）、動作結果（resultFile）、看門狗（alertFile+logFile）可疊加
  function statusView(s, info) {
    if (!info) return { dot: "", lines: ["讀取中…"], alert: "", note: "", last: "" };
    const lines = [];
    let dot = "", alert = "", note = "";
    const sched = info.sched;
    if (sched) {
      if (sched.error) { alert = `排程讀取失敗：${sched.error}`; dot = "bad"; }
      else {
        const kind = sched.lastResultKind;
        lines.push(`上次：${sched.lastRun || "—"}（${RESULT_LABEL[kind] || sched.lastResult}）．下次：${sched.nextRun || "—"}`);
        dot = kind === "failure" ? "bad" : kind === "never" ? "" : "up";
        if (kind === "failure") alert = `上次結果碼 ${sched.lastResult}`;
      }
    }
    const file = info.file;
    if (file) {
      if (file.status) {                       // action_status（resultFile）
        lines.push(`${file.checkedAt ? `上次掃描：${file.checkedAt}` : "尚無掃描紀錄"}．結果：${RESULT_LABEL[file.status] || "未知"}`);
        if (file.status === "failure") { alert = alert || file.message || "執行失敗"; dot = "bad"; }
        else if (file.status === "success") { note = file.message || ""; dot = dot || "up"; }
      } else {                                 // watchdog_status（alertFile/logFile）
        if (file.alert) { alert = file.alert; dot = "bad"; }
        if (file.lastRun) { lines.push(`最近檢查：${file.lastRun}`); dot = dot || "up"; }
        else if (!sched && !file.lastLine) lines.push("尚無巡檢紀錄");
      }
    }
    return { dot, lines, alert, note, last: file?.lastLine || "" };
  }

  function renderStatus(g, s) {
    const v = statusView(s, statusInfo[s.id]);
    return `<div class="card"><div class="dot ${v.dot}"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div>
      <div class="ds">${esc(s.desc ?? "")}</div>
      ${v.lines.map(l => `<div class="sched">${esc(l)}</div>`).join("")}
      ${v.alert ? `<div class="alert">${esc(v.alert)}</div>` : ""}
      ${v.note ? `<div class="oknote">${esc(v.note)}</div>` : ""}
      ${v.last ? `<div class="lastline" title="log 最後一行">${esc(v.last)}</div>` : ""}
    </div>${archiveControl(g, s)}</div>`;
  }

  function renderService(g, s) {
    const up = health[s.id], pending = busy.has(s.id);
    const downAlert = !pending && !up && s.downAlert;
    return `<div class="card"><div class="dot ${pending ? "pending" : up ? "up" : downAlert ? "bad" : ""}"></div>
      <div class="meta">
        ${s.open ? `<button type="button" class="nm" data-open="${esc(s.id)}">${esc(s.name)} ↗</button>` : `<div class="nm">${esc(s.name)}</div>`}
        <div class="ds">${esc(s.desc ?? "")}</div>
        ${downAlert ? `<div class="alert">${esc(s.downAlert)}</div>` : ""}
      </div>
      <button data-act="${esc(s.id)}" class="${up ? "stop" : ""}" ${pending ? "disabled" : ""}>${pending ? "…" : up ? "停止" : "啟動"}</button>
      ${archiveControl(g, s)}
    </div>`;
  }

  function renderCard(g, s) {
    if (isArchived(g, s)) return renderArchived(g, s);
    if (s.kind === "info") return renderInfo(g, s);
    if (s.kind === "status") return renderStatus(g, s);
    return renderService(g, s);
  }

  /// 每張卡的健康分類（群組收合時標題列的摘要用）；資訊卡不計
  function cardState(g, s) {
    if (isArchived(g, s)) return "archived";
    if (s.kind === "info") return "info";
    if (s.kind === "status") { const d = statusView(s, statusInfo[s.id]).dot; return d === "up" ? "up" : d === "bad" ? "bad" : "unknown"; }
    if (busy.has(s.id)) return "pending";
    return health[s.id] ? "up" : s.downAlert ? "bad" : "down";
  }
  const SUMMARY_LABEL = [["up", "運行中"], ["down", "停止"], ["bad", "異常"], ["pending", "處理中"], ["unknown", "未知"], ["archived", "封存"]];
  function groupSummary(g) {
    const count = {};
    g.services.forEach(s => { const k = cardState(g, s); count[k] = (count[k] || 0) + 1; });
    const parts = SUMMARY_LABEL.filter(([k]) => count[k]).map(([k, l]) => `<span class="s-${k}">${count[k]} ${l}</span>`);
    return parts.length ? parts.join("<span class=\"sep\">・</span>") : `${g.services.length} 項`;
  }

  function renderGroup(g) {
    const isCollapsed = collapsed.has(g.name);
    return `<section class="group ${g.archived ? "archived" : ""} ${isCollapsed ? "collapsed" : ""}" data-group="${esc(g.name)}">
      <div class="ghead">
        <button type="button" class="gtoggle" data-toggle="${esc(g.name)}" aria-expanded="${isCollapsed ? "false" : "true"}" title="${isCollapsed ? "展開" : "收合"}">
          <span class="caret" aria-hidden="true">${isCollapsed ? "▸" : "▾"}</span><h2 class="gname">${esc(g.name)}</h2>
        </button>
        ${isCollapsed ? `<span class="gsum">${groupSummary(g)}</span>` : ""}
        <span class="gactions">${archiveControl(g, null)}</span>
      </div>
      ${isCollapsed ? "" : g.services.map(s => renderCard(g, s)).join("")}
    </section>`;
  }

  function render() {
    const root = $("#root");
    const groups = visibleGroups();
    root.innerHTML = groups.length
      ? groups.map(renderGroup).join("")
      : `<div class="empty">沒有可顯示的服務（勾「顯示封存」可看封存項目）</div>`;
    root.querySelectorAll("[data-act]").forEach(b => { b.onclick = () => toggle(b.dataset.act); });
    root.querySelectorAll("[data-open]").forEach(n => { n.onclick = () => openService(n.dataset.open); });
    root.querySelectorAll("[data-toggle]").forEach(b => { b.onclick = () => toggleCollapsed(b.dataset.toggle); });
    root.querySelectorAll("[data-arch-group]").forEach(b => {
      b.onclick = () => {
        const group = b.dataset.archGroup, id = b.dataset.archId ?? null;
        const g = (cfg.groups || []).find(x => x.name === group);
        const s = id ? (g?.services || []).find(x => x.id === id) : null;
        confirming = { group, id, archived: b.dataset.archTo === "1", name: s ? s.name : group };
        render();
      };
    });
    root.querySelectorAll("[data-confirm]").forEach(b => {
      b.onclick = () => {
        if (b.dataset.confirm === "yes" && confirming) void setArchived(confirming);
        else { confirming = null; render(); }
      };
    });
  }

  // ---------- 動作 ----------
  async function setArchived({ group, id, archived }) {
    if (archiving) return;
    setErr("");
    archiving = true; render();
    try {
      cfg = await invoke("set_archived", { group, id, archived });   // Rust 端改寫 services.json 並回傳新設定
      confirming = null;
    } catch (e) { setErr(`封存操作失敗（services.json 未改動）：${e}`); }
    archiving = false; render();
    await Promise.all([refreshHealth(), refreshStatus()]);
  }

  async function openService(id) {
    const s = findService(id);
    if (!s?.open) return;
    setErr("");
    try { await invoke("open_page", { id: "page-" + s.id, url: s.open, title: s.name }); }
    catch (e) { setErr(`${s.name}：${e}`); }
  }

  async function toggle(id) {
    const s = findService(id);
    if (!s) return;
    setErr("");
    busy.add(id); render();
    try {
      if (health[id]) {
        await invoke("stop_service", { id, stop: s.stop });
        await waitFor(s, false, 15);
      } else {
        await invoke("start_service", { id, command: s.start, cwd: s.cwd });
        await waitFor(s, true, 90);   // npx／首次啟動可能要下載，給足時間
      }
    } catch (e) { setErr(`${s.name}: ${e}`); }
    busy.delete(id);
    await refreshHealth();
  }

  async function waitFor(s, want, secs) {
    for (let i = 0; i < secs; i++) {
      if (await invoke("probe", { url: s.healthUrl }) === want) return;
      await new Promise(r => setTimeout(r, 1000));
    }
    throw want ? "啟動了但健康檢查一直不過（看 healthUrl）" : "停止後端口仍有回應";
  }

  // ---------- 狀態更新 ----------
  async function readStatus(s) {
    const info = {};
    if (s.taskName) info.sched = await invoke("scheduled_task_status", { taskName: s.taskName });
    const f = s.statusFiles;
    if (f?.resultFile) info.file = await invoke("action_status", { resultFile: f.resultFile });
    else if (f?.alertFile || f?.logFile) info.file = await invoke("watchdog_status", { alertFile: f.alertFile ?? null, logFile: f.logFile ?? null });
    return info;
  }

  let refreshing = false;
  async function refreshHealth() {
    if (!cfg || refreshing) return;
    refreshing = true;
    try {
      await Promise.allSettled(liveServices().filter(s => !s.kind && !busy.has(s.id)).map(async s => {
        try { health[s.id] = await invoke("probe", { url: s.healthUrl }); }
        catch { delete health[s.id]; }
      }));
      render();
    } finally { refreshing = false; }
  }

  let statusRefreshing = false;
  async function refreshStatus() {
    if (!cfg || statusRefreshing) return;
    statusRefreshing = true;
    try {
      await Promise.allSettled(liveServices().filter(s => s.kind === "status").map(async s => {
        try { statusInfo[s.id] = await readStatus(s); }
        catch (e) { statusInfo[s.id] = { file: { alert: "狀態讀取失敗：" + String(e) } }; }
      }));
      render();
    } finally { statusRefreshing = false; }
  }

  /// 重新從磁碟讀 services.json（手改後按「重新整理」即生效，不必重開）
  async function loadConfig() {
    try {
      cfg = await invoke("load_config");
      const name = (cfg.appName || "").trim() || "Workspace Hub";
      document.title = name;
      $("#app-name").textContent = name;
      render();
      await Promise.all([refreshHealth(), refreshStatus()]);
    } catch (e) { $("#root").textContent = "服務設定載入失敗，可按重新整理重試：" + String(e); }
  }

  // ---------- 待辦 ----------
  let todos = [], todosLoaded = false, todosSaving = false;
  const normalizeTodos = v => Array.isArray(v)
    ? v.filter(t => t && typeof t.text === "string").map(t => ({ id: String(t.id ?? Date.now() + Math.random()), text: t.text, done: !!t.done }))
    : [];
  function renderTodos() {
    const list = $("#todoList");
    if (!todos.length) { list.innerHTML = `<div class="empty">尚無待辦</div>`; return; }
    list.innerHTML = todos.map(t => `<div class="card todo ${t.done ? "done" : ""}" data-id="${esc(t.id)}">
      <input type="checkbox" ${t.done ? "checked" : ""} ${todosSaving ? "disabled" : ""} aria-label="完成">
      <div class="todo-text"></div>
      <button class="del" type="button" aria-label="刪除" ${todosSaving ? "disabled" : ""}>×</button></div>`).join("");
    list.querySelectorAll(".todo").forEach(row => {
      const item = todos.find(t => t.id === row.dataset.id);
      row.querySelector(".todo-text").textContent = item.text;
      row.querySelector("input").onchange = e => persistTodos(todos.map(t => t.id === item.id ? { ...t, done: e.target.checked } : t));
      row.querySelector(".del").onclick = () => persistTodos(todos.filter(t => t.id !== item.id));
    });
  }
  async function persistTodos(next) {
    if (!todosLoaded || todosSaving) return false;
    todosSaving = true; $("#todoAdd").disabled = true; renderTodos();
    try { await invoke("save_todos", { todos: next }); todos = next; return true; }
    catch (e) { setErr("待辦儲存失敗，原資料已保留：" + String(e)); return false; }
    finally { todosSaving = false; $("#todoAdd").disabled = !todosLoaded; renderTodos(); }
  }
  async function addTodo() {
    const input = $("#todoInput"), text = input.value.trim();
    if (!text) return;
    if (await persistTodos([{ id: `${Date.now()}-${Math.random().toString(16).slice(2)}`, text, done: false }, ...todos])) input.value = "";
    input.focus();
  }
  async function loadTodos() {
    try { todos = normalizeTodos(await invoke("load_todos")); todosLoaded = true; renderTodos(); $("#todoAdd").disabled = false; }
    catch (e) { $("#todoList").textContent = "待辦載入失敗，可按重新整理重試：" + String(e); }
  }

  // ---------- 綁定 ----------
  $("#show-archived").onchange = e => { showArchived = e.target.checked; render(); };
  $("#refresh").onclick = () => { if (!todosLoaded) void loadTodos(); return loadConfig(); };
  $("#todoAdd").onclick = () => addTodo().catch(e => setErr("待辦儲存失敗：" + String(e)));
  $("#todoInput").onkeydown = e => { if (e.key === "Enter") $("#todoAdd").click(); };
  $("#todoAdd").disabled = true;
  void loadConfig();
  void loadTodos();
  setInterval(() => { if (!document.hidden) void refreshHealth(); }, PROBE_INTERVAL_MS);
  setInterval(() => { if (!document.hidden) void refreshStatus(); }, STATUS_INTERVAL_MS);

  // 給驗證腳本用：不含 token，只回卡片渲染狀態
  window.__hubDebug = () => ({
    appName: document.title, showArchived, collapsed: [...collapsed], confirming, archiving,
    groups: visibleGroups().map(g => ({ name: g.name, archived: !!g.archived, collapsed: collapsed.has(g.name), summary: groupSummary(g).replace(/<[^>]+>/g, ""), services: g.services.map(s => ({ id: s.id, archived: !!s.archived })) })),
    health: { ...health },
    status: Object.fromEntries(Object.entries(statusInfo).map(([k, v]) => [k, v.sched ? { nextRun: v.sched.nextRun, kind: v.sched.lastResultKind, error: v.sched.error } : "file"])),
  });
})();
