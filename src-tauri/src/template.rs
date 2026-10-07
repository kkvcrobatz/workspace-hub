//! services.json 字串樣板：`{file:<路徑>}` 讀檔內容（去頭尾空白）代入、`{date:<格式>}` 代入本機日期。
//! 用途：開啟連結要帶每次啟動重新產生的 session token、logFile 按日期命名。
//! 讀不到檔（不存在／空）是「服務尚未啟動」的正常狀態，回 Err 讓呼叫端決定怎麼顯示，不 panic。
use chrono::{DateTime, Local};

/// 單一 token 的解析結果；錯誤訊息不含檔案內容，只含路徑。
pub fn resolve(text: &str, now: DateTime<Local>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let token = &after[..end];
        match token.split_once(':') {
            Some(("file", path)) => out.push_str(&read_trimmed(path)?),
            Some(("date", fmt)) => out.push_str(&now.format(&dotnet_to_strftime(fmt)).to_string()),
            _ => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn read_trimmed(path: &str) -> Result<String, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("讀不到 {path}: {e}"))?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(format!("{path} 是空的"));
    }
    Ok(trimmed.to_string())
}

/// .NET 風格日期格式（yyyyMMdd、HHmmss）→ strftime。只支援常用的六種單位，其餘字元原樣保留。
fn dotnet_to_strftime(fmt: &str) -> String {
    const UNITS: [(&str, &str); 6] = [("yyyy", "%Y"), ("MM", "%m"), ("dd", "%d"), ("HH", "%H"), ("mm", "%M"), ("ss", "%S")];
    let mut out = String::new();
    let mut rest = fmt;
    'outer: while !rest.is_empty() {
        for (unit, repl) in UNITS {
            if let Some(tail) = rest.strip_prefix(unit) {
                out.push_str(repl);
                rest = tail;
                continue 'outer;
            }
        }
        let ch = rest.chars().next().unwrap();
        if ch == '%' {
            out.push_str("%%");
        } else {
            out.push(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 7, 21, 5, 9).unwrap()
    }

    #[test]
    fn date_token_uses_dotnet_format() {
        assert_eq!(resolve(r"E:\logs\{date:yyyyMMdd}-record.log", at()).unwrap(), r"E:\logs\20261007-record.log");
        assert_eq!(resolve("{date:yyyy-MM-dd HH:mm:ss}", at()).unwrap(), "2026-10-07 21:05:09");
    }

    #[test]
    fn file_token_reads_trimmed_content() {
        let dir = std::env::temp_dir().join(format!("hub-template-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        std::fs::write(&path, "  abc123\r\n").unwrap();
        let url = format!("http://127.0.0.1:4327/?token={{file:{}}}", path.display());
        assert_eq!(resolve(&url, at()).unwrap(), "http://127.0.0.1:4327/?token=abc123");
        std::fs::write(&path, "   \n").unwrap();
        assert!(resolve(&url, at()).unwrap_err().contains("是空的"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_is_err_without_panicking() {
        let err = resolve(r"{file:E:\definitely\missing\token}", at()).unwrap_err();
        assert!(err.contains("讀不到"));
    }

    #[test]
    fn unknown_or_unterminated_braces_pass_through() {
        assert_eq!(resolve("a{b}c{", at()).unwrap(), "a{b}c{");
        assert_eq!(resolve("{x:y}", at()).unwrap(), "{x:y}");
    }
}
