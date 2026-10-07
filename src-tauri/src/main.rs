#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 工作列分組用的 AppUserModelID。必須和 tauri.conf.json 的 `identifier`
/// 以及 build 腳本寫進捷徑（System.AppUserModel.ID）的字串完全一致：
/// Windows 以它（而非 exe 路徑）合併「釘選的捷徑」與「執行中的視窗」，
/// 否則交付路徑一換，工作列就會出現第二個圖示（2026-10-07 實例）。
/// Tauri 2（2.11.5 查證）自己不會呼叫 SetCurrentProcessExplicitAppUserModelID，所以這裡在建視窗前設。
const APP_USER_MODEL_ID: &str = "tw.kyle.agenthub";

#[link(name = "shell32")]
extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
}

fn set_app_user_model_id() {
    let wide: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain(std::iter::once(0)).collect();
    // 失敗（HRESULT < 0）只影響工作列分組，不影響功能；static 字串不會失敗，忽略回傳值即可。
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
}

fn main() {
    set_app_user_model_id();
    agent_hub_lib::run()
}
