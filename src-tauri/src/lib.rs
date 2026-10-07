// Workspace Hub：整個 workspace 的統一入口（設定驅動，見 ../../services.json；名稱由 appName 決定）
// 關主視窗縮到系統匣；退出桌面程式保留背景服務。
mod archive;
mod schtasks;
mod template;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SERVICES_PATH: &str = r"E:\Kyle\Workspace\agent-harness\hub\services.json";
const TODOS_PATH: &str = r"E:\Kyle\Workspace\agent-harness\hub\data\todos.json";
const DEFAULT_APP_NAME: &str = "Workspace Hub";

fn services_path() -> String {
    std::env::var("HUB_SERVICES").unwrap_or_else(|_| SERVICES_PATH.to_string())
}

fn read_config() -> Result<serde_json::Value, String> {
    let path = services_path();
    let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("services.json 解析失敗: {e}"))
}

/// 視窗標題、系統匣、捷徑名都用它；單實例鎖與設定路徑不跟著變。
fn app_name(cfg: &serde_json::Value) -> String {
    cfg["appName"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_APP_NAME).to_string()
}

/// 把 services.json 字串裡的 `{file:}` / `{date:}` 樣板展開（見 template.rs）。
fn expand(text: &str) -> Result<String, String> {
    template::resolve(text, chrono::Local::now())
}

/// hub 自己 spawn 的服務：id -> 包裹程序 pid（停止時 taskkill /T 殺整棵樹）
#[derive(Default)]
struct Spawned(Mutex<HashMap<String, u32>>);

// 所有 command 一律 async：同步 command 在 Windows 主執行緒上執行，
// 在裡面開視窗會死鎖事件迴圈（視窗一片白、整個 app 凍住）；async 走工作執行緒池。
#[tauri::command]
async fn load_config() -> Result<serde_json::Value, String> {
    read_config()
}

#[tauri::command]
async fn load_todos() -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(TODOS_PATH) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("todos.json 解析失敗: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!([])),
        Err(e) => Err(format!("{TODOS_PATH}: {e}")),
    }
}

#[tauri::command]
async fn save_todos(todos: serde_json::Value) -> Result<(), String> {
    if let Some(parent) = Path::new(TODOS_PATH).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建立 todos 目錄失敗: {e}"))?;
    }
    let text = serde_json::to_string_pretty(&todos).map_err(|e| format!("todos 序列化失敗: {e}"))?;
    std::fs::write(TODOS_PATH, text).map_err(|e| format!("{TODOS_PATH}: {e}"))
}

/// 封存／取消封存一個群組（`id` 為 None）或服務：純文字改 services.json 只切 `archived`，
/// 寫前先存 services.json.bak。回傳改後的完整設定讓前端即時更新。
#[tauri::command]
async fn set_archived(group: String, id: Option<String>, archived: bool) -> Result<serde_json::Value, String> {
    let path = services_path();
    let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let next = archive::set_archived(&text, &group, id.as_deref(), archived)?;
    if next != text {
        let bak = format!("{path}.bak");
        fs::write(&bak, &text).map_err(|e| format!("寫備份 {bak} 失敗，未改動: {e}"))?;
        fs::write(&path, &next).map_err(|e| format!("{path}: {e}"))?;
    }
    serde_json::from_str(&next).map_err(|e| format!("services.json 解析失敗: {e}"))
}

/// 健康檢查：打真實 API 路由拿 2xx 才算活（端口有回應 ≠ 服務健康）。
/// URL 可含樣板；樣板展不開（例如 token 檔還沒生成）視同未啟動。
#[tauri::command]
async fn probe(url: String) -> bool {
    expand(&url).map(|u| http_request(&u, "GET").is_some()).unwrap_or(false)
}

/// Windows 工作排程器任務的唯讀狀態（上次執行／結果／下次執行），走 schtasks 不走 WMI。
#[tauri::command]
async fn scheduled_task_status(task_name: String) -> serde_json::Value {
    schtasks::query(&task_name)
}

/// 排程任務類背景程序的唯讀狀態（無 HTTP 端點可打）：alertFile 存在＝紅燈＋內容摘要；
/// logFile 最後一行取時間戳當「最近檢查」。兩者都是純檔案讀取，讀不到就回「未知」而非報錯，
/// 避免看門狗本身還沒跑過第一輪時，卡片直接顯示錯誤。
#[tauri::command]
async fn watchdog_status(alert_file: Option<String>, log_file: Option<String>) -> serde_json::Value {
    let alert_file = alert_file.as_deref().map(expand).and_then(Result::ok).unwrap_or_default();
    let log_file = log_file.as_deref().map(expand).and_then(Result::ok).unwrap_or_default();
    let alert = std::fs::read_to_string(&alert_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let last_run = std::fs::read_to_string(&log_file)
        .ok()
        .and_then(|text| {
            // 只看最後一行非空內容：排程 log 中間偶有 [完成] 之類的行，往回掃會抓到不相干的字
            text.lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .filter(|l| l.starts_with('['))
                .map(|l| l.to_string())
        })
        .and_then(|line| {
            line.strip_prefix('[')
                .and_then(|rest| rest.split(']').next())
                .map(|ts| ts.to_string())
        });
    // 排程任務的 log 沒有 [時間] 前綴：取最後一行非空內容當「最近紀錄」（截斷，避免卡片爆版）
    let last_line = std::fs::read_to_string(&log_file).ok().and_then(|text| {
        text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(160).collect::<String>())
    });
    serde_json::json!({ "alert": alert, "lastRun": last_run, "lastLine": last_line, "logFile": log_file })
}

#[tauri::command]
async fn action_status(result_file: String) -> serde_json::Value {
    let result_file = expand(&result_file).unwrap_or(result_file);
    match std::fs::read_to_string(&result_file)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    {
        Some(value) => value,
        None => serde_json::json!({ "status": "unknown", "message": "尚無執行紀錄" }),
    }
}

#[tauri::command]
async fn start_service(state: State<'_, Spawned>, id: String, command: String, cwd: String) -> Result<u32, String> {
    let child = Command::new("cmd")
        .args(["/c", &command])
        .current_dir(&cwd)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("啟動失敗: {e}"))?;
    let pid = child.id();
    state.0.lock().unwrap().insert(id, pid);
    Ok(pid)
}

#[tauri::command]
async fn stop_service(state: State<'_, Spawned>, id: String, stop: serde_json::Value) -> Result<String, String> {
    stop_service_inner(&state, &id, &stop)
}

fn stop_service_inner(state: &Spawned, id: &str, stop: &serde_json::Value) -> Result<String, String> {
    match stop["type"].as_str() {
        Some("http") => {
            let url = stop["url"].as_str().ok_or("stop.url 缺失")?;
            http_request(url, "POST").ok_or_else(|| "shutdown 端點沒有回應".to_string())?;
            state.0.lock().unwrap().remove(id);
            Ok("已送出 shutdown".into())
        }
        Some("kill") => {
            // 優先殺自己 spawn 的整棵樹；外部啟動的按命令列 pattern 找（各自 /T 殺樹，
            // 因為 npx 會生 cmd→node→引擎 多層，殺單點會留孤兒續佔端口）
            if let Some(pid) = state.0.lock().unwrap().remove(id) {
                let _ = taskkill_tree(pid);
            }
            if let Some(pattern) = stop["pattern"].as_str() {
                kill_by_cmdline(pattern, stop["processName"].as_str())?;
            }
            Ok("已終止".into())
        }
        _ => Err("未知的 stop.type".into()),
    }
}

#[tauri::command]
async fn open_page(app: AppHandle, id: String, url: String, title: String, external: Option<bool>) -> Result<(), String> {
    // external:true（例如 claude.ai 的 artifact）直接交給系統瀏覽器，沿用使用者已登入的 session
    if external.unwrap_or(false) {
        Command::new("cmd").args(["/c", "start", "", &url]).creation_flags(CREATE_NO_WINDOW).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    if let Some(w) = app.get_webview_window(&id) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    // 樣板展不開（token 檔不存在）＝服務還沒啟動；錯誤訊息只含路徑，不含 token 內容
    let url = expand(&url).map_err(|e| format!("尚未啟動（{e}）"))?;
    let parsed: tauri::Url = url.parse().map_err(|e| format!("URL 不合法: {e}"))?;
    let origin = (parsed.host_str().map(String::from), parsed.port_or_known_default());
    // webview 預設丟棄 target=_blank：注入腳本把 _blank/window.open 轉成本視窗導航，
    // 再由 on_navigation 攔截「跨 origin」的導航改開系統瀏覽器（同 origin 照常）。
    WebviewWindowBuilder::new(&app, &id, WebviewUrl::External(parsed))
        .title(&title)
        .inner_size(1240.0, 820.0)
        .initialization_script(BLANK_LINK_SHIM)
        .on_navigation(move |nav_url| {
            let same = (nav_url.host_str().map(String::from), nav_url.port_or_known_default()) == origin;
            if !same {
                let _ = Command::new("cmd")
                    .args(["/c", "start", "", nav_url.as_str()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn();
            }
            same
        })
        .build()
        .map_err(|e| format!("開視窗失敗: {e}"))?;
    Ok(())
}

const BLANK_LINK_SHIM: &str = r#"(function(){
  const nav = u => { try { location.href = u } catch (e) {} };
  window.open = u => { if (u) nav(u); return null; };
  document.addEventListener('click', e => {
    const a = e.target && e.target.closest && e.target.closest('a[target="_blank"]');
    if (a && a.href) { e.preventDefault(); nav(a.href); }
  }, true);
})();"#;

fn taskkill_tree(pid: u32) -> Option<()> {
    Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .map(|_| ())
}

/// 按命令列 pattern 殺程序。
///
/// `process_name`（services.json 的 `stop.processName`）是 2026-07-30 補的安全閘：
/// 原本只比對命令列，任何**提到**目標腳本檔名的程序都會被 `taskkill /T /F`——
/// 實測會命中正在執行含該檔名指令的 bash shell、編輯器、其他 agent 的派工 prompt。
/// 對完整命令列做 regex 本質上不可靠，因為別人的命令列可以包含你的 pattern 文字。
/// 加上程序名約束後，只有真正是該執行檔的程序會被考慮，整類誤殺就消失了。
/// 沒設 processName 時維持原行為（相容既有服務卡），但新服務都應該設。
fn kill_by_cmdline(pattern: &str, process_name: Option<&str>) -> Result<(), String> {
    let name_filter = match process_name {
        Some(n) => format!(" -and $_.Name -eq '{}'", n.replace('\'', "''")),
        None => String::new(),
    };
    // PowerShell 找命令列符合 pattern 的程序，逐一殺樹；排除 hub 自己
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -match '{}'{} -and $_.ProcessId -ne {} }} | ForEach-Object {{ taskkill /T /F /PID $_.ProcessId 2>$null }}",
        pattern.replace('\'', "''"),
        name_filter,
        std::process::id()
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("kill 失敗: {e}"))?;
    Ok(())
}

/// 封存的群組／服務不做任何自動動作
fn run_startup_actions() {
    let Ok(cfg) = read_config() else { return; };
    if let Some(groups) = cfg["groups"].as_array() {
        for group in groups {
            if group["archived"].as_bool().unwrap_or(false) {
                continue;
            }
            if let Some(services) = group["services"].as_array() {
                for service in services {
                    if service["archived"].as_bool().unwrap_or(false) {
                        continue;
                    }
                    if let Some(action) = service["runOnHubStart"].as_object() {
                        run_startup_action(service["id"].as_str().unwrap_or("startup-action"), action);
                    }
                }
            }
        }
    }
}

fn run_startup_action(id: &str, action: &serde_json::Map<String, serde_json::Value>) {
    let command = action.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = action.get("cwd").and_then(|v| v.as_str()).unwrap_or(r"E:\Kyle\Workspace\agent-harness");
    let state_file = action.get("stateFile").and_then(|v| v.as_str()).unwrap_or("");
    let result_file = action.get("resultFile").and_then(|v| v.as_str()).unwrap_or("");
    if command.is_empty() || state_file.is_empty() || result_file.is_empty() {
        return;
    }
    let today = local_date_string().unwrap_or_else(|| "unknown".to_string());
    if action.get("oncePerDay").and_then(|v| v.as_bool()).unwrap_or(false)
        && last_success_date(state_file).as_deref() == Some(today.as_str())
    {
        write_action_result(result_file, "success", "今日已成功掃描，略過重複執行", None);
        return;
    }
    if let Some(url) = action.get("dependsOnHealthUrl").and_then(|v| v.as_str()) {
        let wait_seconds = action.get("waitSeconds").and_then(|v| v.as_u64()).unwrap_or(90);
        if !wait_for_health(url, wait_seconds) {
            write_action_result(result_file, "failure", "依賴服務尚未通過健康檢查，未執行動作", None);
            return;
        }
    }
    let output = Command::new("cmd")
        .args(["/c", command])
        .current_dir(cwd)
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let details = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let _ = write_state_success(state_file, &today);
            write_action_result(result_file, "success", "掃描成功", Some(details));
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let details = if stderr.is_empty() { stdout } else { stderr };
            write_action_result(
                result_file,
                "failure",
                &format!("{id} 退出碼 {}", out.status.code().unwrap_or(-1)),
                Some(details),
            );
        }
        Err(e) => {
            write_action_result(result_file, "failure", &format!("啟動失敗: {e}"), None);
        }
    }
}

fn wait_for_health(url: &str, seconds: u64) -> bool {
    for _ in 0..seconds {
        if http_request(url, "GET").is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    false
}

fn local_date_string() -> Option<String> {
    Some(chrono::Local::now().format("%Y-%m-%d").to_string())
}

fn last_success_date(path: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    json["lastSuccessDate"].as_str().map(|s| s.to_string())
}

fn write_state_success(path: &str, date: &str) -> Option<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let value = serde_json::json!({
        "lastSuccessDate": date,
        "lastSuccessAt": local_timestamp_string().unwrap_or_else(|| date.to_string())
    });
    fs::write(path, serde_json::to_string_pretty(&value).ok()?).ok()
}

fn write_action_result(path: &str, status: &str, message: &str, details: Option<String>) -> Option<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let value = serde_json::json!({
        "status": status,
        "checkedAt": local_timestamp_string().unwrap_or_else(|| "unknown".to_string()),
        "message": message,
        "details": details.unwrap_or_default()
    });
    fs::write(path, serde_json::to_string_pretty(&value).ok()?).ok()
}

fn local_timestamp_string() -> Option<String> {
    Some(chrono::Local::now().to_rfc3339())
}

/// 極簡 HTTP client（只打 127.0.0.1，避免整套 reqwest 依賴）；2xx 回 Some
fn http_request(url: &str, method: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let (hostport, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let addr: std::net::SocketAddr = hostport.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(1200)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(2000))).ok()?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    if text.starts_with("HTTP/1.1 2") || text.starts_with("HTTP/1.0 2") {
        Some(text.into_owned())
    } else {
        None
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| show_main(app)))
        .manage(Spawned::default())
        .invoke_handler(tauri::generate_handler![
            load_config,
            load_todos,
            save_todos,
            set_archived,
            probe,
            scheduled_task_status,
            watchdog_status,
            action_status,
            start_service,
            stop_service,
            open_page
        ])
        .setup(|app| {
            let name = read_config().map(|cfg| app_name(&cfg)).unwrap_or_else(|_| DEFAULT_APP_NAME.to_string());
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&name);
            }
            let open = MenuItemBuilder::with_id("open", format!("開啟 {name}")).build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "退出桌面程式（背景服務繼續執行）").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;
            std::thread::spawn(run_startup_actions);
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip(&name)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 關主視窗＝縮到系統匣（服務照跑）；真正退出走 tray 選單
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Workspace Hub 啟動失敗");
}
