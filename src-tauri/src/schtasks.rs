//! Windows 工作排程器任務的唯讀狀態：`schtasks /query /tn <名稱> /fo csv /v`。
//! 刻意不用 WMI／CIM（2026-10-07 本機 WMI 曾卡死）。
//! schtasks 的輸出走 OEM 字碼頁（本機 950），Rust 無法直接解碼，所以經 Windows PowerShell 轉成 UTF-8：
//! 先用主控台預設編碼接住輸出，再把 OutputEncoding 切成 UTF-8 印出前兩行（表頭＋第一個觸發列）。
//! 欄位用**位置**取（/v 的 CSV 欄序固定），不靠表頭文字——表頭會隨語系變成中文。
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const COL_NEXT_RUN: usize = 2;
const COL_STATUS: usize = 3;
const COL_LAST_RUN: usize = 5;
const COL_LAST_RESULT: usize = 6;

/// 查一個任務；找不到或 schtasks 失敗時回 `{"error": ...}`，不拋錯（卡片顯示而非整頁壞掉）。
pub fn query(task_name: &str) -> serde_json::Value {
    if task_name.is_empty() || task_name.contains('\'') || task_name.contains('"') {
        return serde_json::json!({ "error": "taskName 不合法" });
    }
    let script = format!(
        "$lines = schtasks /query /tn '{task_name}' /fo csv /v 2>&1; $code = $LASTEXITCODE; \
         [Console]::OutputEncoding = [Text.Encoding]::UTF8; \
         if ($code -ne 0) {{ Write-Output ('ERR ' + ($lines -join ' ')) }} else {{ $lines | Select-Object -First 2 }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) => parse_output(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => serde_json::json!({ "error": format!("schtasks 無法執行: {e}") }),
    }
}

/// 純函式，方便測：輸入 PowerShell 轉出的兩行 CSV（或 `ERR ...`）。
pub fn parse_output(text: &str) -> serde_json::Value {
    let text = text.trim_start_matches('\u{feff}').trim();
    if let Some(msg) = text.strip_prefix("ERR ") {
        return serde_json::json!({ "error": msg.trim() });
    }
    let Some(row) = text.lines().nth(1) else {
        return serde_json::json!({ "error": "schtasks 沒有回傳任務列" });
    };
    let cols = parse_csv_row(row);
    let get = |i: usize| cols.get(i).cloned().unwrap_or_default();
    let last_result = get(COL_LAST_RESULT);
    serde_json::json!({
        "lastRun": get(COL_LAST_RUN),
        "lastResult": last_result,
        "lastResultKind": classify_result(&last_result),
        "nextRun": get(COL_NEXT_RUN),
        "status": get(COL_STATUS),
    })
}

/// Last Result 的語意：0 成功；267009（0x41301）仍在執行；267011（0x41303）尚未執行過；其餘失敗。
pub fn classify_result(code: &str) -> &'static str {
    match code.trim() {
        "0" => "success",
        "267009" => "running",
        "267011" | "" | "N/A" => "never",
        _ => "failure",
    }
}

/// 只處理 schtasks 的輸出形狀：每欄都有雙引號、引號內以 `""` 轉義、逗號分隔。
fn parse_csv_row(row: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => cols.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cols.push(cur);
    cols
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\"HostName\",\"TaskName\",\"Next Run Time\",\"Status\",\"Logon Mode\",\"Last Run Time\",\"Last Result\",\"Author\"\r\n\
\"PHOENIX\",\"\\trading-data-fetch-daily\",\"2026/10/8 下午 09:00:00\",\"Ready\",\"Interactive only\",\"2026/10/7 下午 09:00:00\",\"0\",\"N/A\"\r\n\
\"PHOENIX\",\"\\trading-data-fetch-daily\",\"2026/10/9 下午 09:00:00\",\"Ready\",\"Interactive only\",\"2026/10/7 下午 09:00:00\",\"0\",\"N/A\"\r\n";

    #[test]
    fn parses_first_trigger_row_by_position() {
        let v = parse_output(SAMPLE);
        assert_eq!(v["nextRun"], "2026/10/8 下午 09:00:00");
        assert_eq!(v["lastRun"], "2026/10/7 下午 09:00:00");
        assert_eq!(v["lastResult"], "0");
        assert_eq!(v["lastResultKind"], "success");
        assert_eq!(v["status"], "Ready");
    }

    #[test]
    fn error_line_becomes_error_field() {
        let v = parse_output("ERR 錯誤: 系統找不到指定的檔案。");
        assert!(v["error"].as_str().unwrap().contains("找不到"));
        assert!(parse_output("\"only-header\"").get("error").is_some());
    }

    #[test]
    fn result_codes_are_classified() {
        assert_eq!(classify_result("0"), "success");
        assert_eq!(classify_result("267009"), "running");
        assert_eq!(classify_result("267011"), "never");
        assert_eq!(classify_result("1"), "failure");
    }

    #[test]
    fn csv_row_handles_escaped_quotes() {
        assert_eq!(parse_csv_row("\"a\",\"b \"\"q\"\" c\",\"\""), vec!["a", "b \"q\" c", ""]);
    }

    #[test]
    fn rejects_quote_injection_in_task_name() {
        assert!(query("x' ; Write-Output 'pwned").get("error").is_some());
    }

    /// 真的呼叫本機 schtasks：用 Windows 內建的一個排程任務確認編碼管線與欄位位置沒有錯位。
    #[test]
    fn live_query_decodes_localized_output() {
        let v = query(r"\Microsoft\Windows\Defrag\ScheduledDefrag");
        if v.get("error").is_some() {
            eprintln!("skip: {v}");
            return;
        }
        assert!(!v["status"].as_str().unwrap().is_empty());
        assert!(!v["lastResult"].as_str().unwrap().contains('\u{fffd}'));
    }
}
