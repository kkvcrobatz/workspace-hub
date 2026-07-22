// Agent Hub：本機服務啟動器（設定驅動，見 ../../services.json）
// 職責邊界：hub 只管「啟動/停止/看狀態/開頁面」；結束 hub 不影響任何服務。
use std::collections::HashMap;
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

/// hub 自己 spawn 的服務：id -> 包裹程序 pid（停止時 taskkill /T 殺整棵樹）
#[derive(Default)]
struct Spawned(Mutex<HashMap<String, u32>>);

// 所有 command 一律 async：同步 command 在 Windows 主執行緒上執行，
// 在裡面開視窗會死鎖事件迴圈（視窗一片白、整個 app 凍住）；async 走工作執行緒池。
#[tauri::command]
async fn load_config() -> Result<serde_json::Value, String> {
    let path = std::env::var("HUB_SERVICES").unwrap_or_else(|_| SERVICES_PATH.to_string());
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("services.json 解析失敗: {e}"))
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

/// 健康檢查：打真實 API 路由拿 2xx 才算活（端口有回應 ≠ 服務健康）
#[tauri::command]
async fn probe(url: String) -> bool {
    http_request(&url, "GET").is_some()
}

/// 排程任務類背景程序的唯讀狀態（無 HTTP 端點可打）：alertFile 存在＝紅燈＋內容摘要；
/// logFile 最後一行取時間戳當「最近檢查」。兩者都是純檔案讀取，讀不到就回「未知」而非報錯，
/// 避免看門狗本身還沒跑過第一輪時，卡片直接顯示錯誤。
#[tauri::command]
async fn watchdog_status(alert_file: String, log_file: String) -> serde_json::Value {
    let alert = std::fs::read_to_string(&alert_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let last_run = std::fs::read_to_string(&log_file)
        .ok()
        .and_then(|text| {
            text.lines()
                .rev()
                .find(|l| l.trim_start().starts_with('['))
                .map(|l| l.to_string())
        })
        .and_then(|line| {
            line.strip_prefix('[')
                .and_then(|rest| rest.split(']').next())
                .map(|ts| ts.to_string())
        });
    serde_json::json!({ "alert": alert, "lastRun": last_run })
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
    match stop["type"].as_str() {
        Some("http") => {
            let url = stop["url"].as_str().ok_or("stop.url 缺失")?;
            http_request(url, "POST").ok_or_else(|| "shutdown 端點沒有回應".to_string())?;
            state.0.lock().unwrap().remove(&id);
            Ok("已送出 shutdown".into())
        }
        Some("kill") => {
            // 優先殺自己 spawn 的整棵樹；外部啟動的按命令列 pattern 找（各自 /T 殺樹，
            // 因為 npx 會生 cmd→node→引擎 多層，殺單點會留孤兒續佔端口）
            if let Some(pid) = state.0.lock().unwrap().remove(&id) {
                let _ = taskkill_tree(pid);
            }
            if let Some(pattern) = stop["pattern"].as_str() {
                kill_by_cmdline(pattern)?;
            }
            Ok("已終止".into())
        }
        _ => Err("未知的 stop.type".into()),
    }
}

#[tauri::command]
async fn open_page(app: AppHandle, id: String, url: String, title: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&id) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
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

fn kill_by_cmdline(pattern: &str) -> Result<(), String> {
    // PowerShell 找命令列符合 pattern 的程序，逐一殺樹；排除 hub 自己
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -match '{}' -and $_.ProcessId -ne {} }} | ForEach-Object {{ taskkill /T /F /PID $_.ProcessId 2>$null }}",
        pattern.replace('\'', "''"),
        std::process::id()
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("kill 失敗: {e}"))?;
    Ok(())
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
            probe,
            watchdog_status,
            start_service,
            stop_service,
            open_page
        ])
        .setup(|app| {
            let open = MenuItemBuilder::with_id("open", "開啟 Agent Hub").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "結束 Hub（服務不受影響）").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Agent Hub")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
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
        .expect("Agent Hub 啟動失敗");
}
