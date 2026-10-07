// Workspace Hub 儀表板：services.json 驅動的卡片（一般服務／狀態卡／資訊卡／封存卡）＋個人待辦。
// 樣板（{file:}/{date:}）一律在 Rust 端展開，前端看不到 token 內容。
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

  let cfg = null;
  let showArchived = false;
  const busy = new Set();      // 啟停過渡中的服務 id
  const health = {};           // id -> bool
  const statusInfo = {};       // id -> 狀態卡資料

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

  function renderArchived(s) {
    return `<div class="card archived"><div class="dot none"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div><div class="ds">${esc(s.desc ?? "")}</div></div></div>`;
  }
  function renderInfo(s) {
    return `<div class="card info"><div class="dot none"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div><div class="ds">${esc(s.desc ?? "")}</div>
      ${s.note ? `<div class="lastline">${esc(s.note)}</div>` : ""}</div></div>`;
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

  function renderStatus(s) {
    const v = statusView(s, statusInfo[s.id]);
    return `<div class="card"><div class="dot ${v.dot}"></div><div class="meta">
      <div class="nm">${esc(s.name)}</div>
      <div class="ds">${esc(s.desc ?? "")}</div>
      ${v.lines.map(l => `<div class="sched">${esc(l)}</div>`).join("")}
      ${v.alert ? `<div class="alert">${esc(v.alert)}</div>` : ""}
      ${v.note ? `<div class="oknote">${esc(v.note)}</div>` : ""}
      ${v.last ? `<div class="lastline" title="log 最後一行">${esc(v.last)}</div>` : ""}
    </div></div>`;
  }

  function renderService(s) {
    const up = health[s.id], pending = busy.has(s.id);
    const downAlert = !pending && !up && s.downAlert;
    return `<div class="card"><div class="dot ${pending ? "pending" : up ? "up" : downAlert ? "bad" : ""}"></div>
      <div class="meta">
        ${s.open ? `<button type="button" class="nm" data-open="${esc(s.id)}">${esc(s.name)} ↗</button>` : `<div class="nm">${esc(s.name)}</div>`}
        <div class="ds">${esc(s.desc ?? "")}</div>
        ${downAlert ? `<div class="alert">${esc(s.downAlert)}</div>` : ""}
      </div>
      <button data-act="${esc(s.id)}" class="${up ? "stop" : ""}" ${pending ? "disabled" : ""}>${pending ? "…" : up ? "停止" : "啟動"}</button>
    </div>`;
  }

  function renderCard(g, s) {
    if (isArchived(g, s)) return renderArchived(s);
    if (s.kind === "info") return renderInfo(s);
    if (s.kind === "status") return renderStatus(s);
    return renderService(s);
  }

  function render() {
    const root = $("#root");
    const groups = visibleGroups();
    root.innerHTML = groups.length
      ? groups.map(g => `<section class="group ${g.archived ? "archived" : ""}"><h2 class="gname">${esc(g.name)}</h2>${g.services.map(s => renderCard(g, s)).join("")}</section>`).join("")
      : `<div class="empty">沒有可顯示的服務（勾「顯示封存」可看封存項目）</div>`;
    root.querySelectorAll("[data-act]").forEach(b => { b.onclick = () => toggle(b.dataset.act); });
    root.querySelectorAll("[data-open]").forEach(n => { n.onclick = () => openService(n.dataset.open); });
  }

  // ---------- 動作 ----------
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
  $("#refresh").onclick = () => { if (!todosLoaded) void loadTodos(); return cfg ? Promise.all([refreshHealth(), refreshStatus()]) : loadConfig(); };
  $("#todoAdd").onclick = () => addTodo().catch(e => setErr("待辦儲存失敗：" + String(e)));
  $("#todoInput").onkeydown = e => { if (e.key === "Enter") $("#todoAdd").click(); };
  $("#todoAdd").disabled = true;
  void loadConfig();
  void loadTodos();
  setInterval(() => { if (!document.hidden) void refreshHealth(); }, PROBE_INTERVAL_MS);
  setInterval(() => { if (!document.hidden) void refreshStatus(); }, STATUS_INTERVAL_MS);

  // 給驗證腳本用：不含 token，只回卡片渲染狀態
  window.__hubDebug = () => ({ appName: document.title, showArchived, groups: visibleGroups().map(g => ({ name: g.name, archived: !!g.archived, services: g.services.map(s => s.id) })), health: { ...health }, status: Object.fromEntries(Object.entries(statusInfo).map(([k, v]) => [k, v.sched ? { nextRun: v.sched.nextRun, kind: v.sched.lastResultKind, error: v.sched.error } : "file"])) });
})();
