//! services.json 的「封存」切換：純文字層級改檔，只動目標群組／服務的 `archived` 成員，
//! 其餘字元（縮排、單行物件、`_readme`、鍵順序、行尾）一律原樣保留，讓檔案仍可手改且 diff 乾淨。
//!
//! 作法：掃描 JSON 文字找出目標物件的字元區間（群組＝`groups[]` 內 `name` 相符的物件；服務＝該群組
//! `services[]` 內 `id` 相符的物件），再在該物件的第一層成員中：
//! - 封存：已有 `archived` → 把值改成 `true`；沒有 → 在 `name`（或 `id`、或第一個成員）之後插入一行
//! - 取消封存：有 `archived` → 整個成員連同分隔逗號移除（沒有該鍵＝未封存，是預設狀態）
//!
//! 不用 serde 重新序列化：pretty printer 會把 `"stop": { ... }` 這類單行物件展開成多行，改一個布林
//! 會讓整個檔案的 diff 爆掉。

/// 物件的一個第一層成員：鍵名與（鍵起點、值起點、值終點＝值最後一個字元之後）的位移。
#[derive(Debug, Clone, PartialEq)]
struct Member {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

/// 跳過空白，回傳下一個非空白字元的位移。
fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// 從 `"` 開始掃到字串結尾，回傳結尾 `"` 之後的位移。
fn skip_string(bytes: &[u8], start: usize) -> Result<usize, String> {
    debug_assert_eq!(bytes[start], b'"');
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("JSON 字串沒有結尾引號".into())
}

/// 從任一值的第一個字元掃到值結尾（物件／陣列配對括號、字串、數字與字面值）。
fn skip_value(bytes: &[u8], start: usize) -> Result<usize, String> {
    match bytes.get(start) {
        Some(b'"') => skip_string(bytes, start),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0usize;
            let mut i = start;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => {
                        i = skip_string(bytes, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Err("JSON 括號沒有配對".into())
        }
        Some(_) => {
            let mut i = start;
            while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                i += 1;
            }
            Ok(i)
        }
        None => Err("JSON 意外結束".into()),
    }
}

/// 列出物件（`{` 位於 `obj_start`）的第一層成員。
fn members(text: &str, obj_start: usize) -> Result<Vec<Member>, String> {
    let bytes = text.as_bytes();
    if bytes.get(obj_start) != Some(&b'{') {
        return Err("預期的位置不是物件".into());
    }
    let mut out = Vec::new();
    let mut i = skip_ws(bytes, obj_start + 1);
    loop {
        match bytes.get(i) {
            Some(b'}') => return Ok(out),
            Some(b'"') => {
                let key_start = i;
                let key_end = skip_string(bytes, i)?;
                let key = serde_json::from_str::<String>(&text[key_start..key_end]).map_err(|e| format!("鍵名解析失敗: {e}"))?;
                let colon = skip_ws(bytes, key_end);
                if bytes.get(colon) != Some(&b':') {
                    return Err("鍵後面不是冒號".into());
                }
                let value_start = skip_ws(bytes, colon + 1);
                let value_end = skip_value(bytes, value_start)?;
                out.push(Member { key, key_start, value_start, value_end });
                i = skip_ws(bytes, value_end);
                match bytes.get(i) {
                    Some(b',') => i = skip_ws(bytes, i + 1),
                    Some(b'}') => return Ok(out),
                    _ => return Err("成員之後不是逗號或右大括號".into()),
                }
            }
            _ => return Err("物件內容格式不符".into()),
        }
    }
}

/// 列出陣列（`[` 位於 `arr_start`）的每個元素的（起點、終點）。
fn elements(text: &str, arr_start: usize) -> Result<Vec<(usize, usize)>, String> {
    let bytes = text.as_bytes();
    if bytes.get(arr_start) != Some(&b'[') {
        return Err("預期的位置不是陣列".into());
    }
    let mut out = Vec::new();
    let mut i = skip_ws(bytes, arr_start + 1);
    loop {
        match bytes.get(i) {
            Some(b']') => return Ok(out),
            Some(_) => {
                let end = skip_value(bytes, i)?;
                out.push((i, end));
                i = skip_ws(bytes, end);
                match bytes.get(i) {
                    Some(b',') => i = skip_ws(bytes, i + 1),
                    Some(b']') => return Ok(out),
                    _ => return Err("元素之後不是逗號或右中括號".into()),
                }
            }
            None => return Err("陣列沒有結尾".into()),
        }
    }
}

fn member_string(text: &str, m: &Member) -> Option<String> {
    serde_json::from_str::<String>(&text[m.value_start..m.value_end]).ok()
}

/// 在 `obj_start` 的物件裡找名為 `key`、字串值等於 `want` 的子物件（於 `array_key` 陣列內）。
fn find_in_array(text: &str, obj_start: usize, array_key: &str, key: &str, want: &str) -> Result<Option<usize>, String> {
    let Some(arr) = members(text, obj_start)?.into_iter().find(|m| m.key == array_key) else { return Ok(None); };
    for (start, _) in elements(text, arr.value_start)? {
        if text.as_bytes()[start] != b'{' {
            continue;
        }
        if members(text, start)?.iter().any(|m| m.key == key && member_string(text, m).as_deref() == Some(want)) {
            return Ok(Some(start));
        }
    }
    Ok(None)
}

/// 偵測檔案的換行與某物件第一層成員使用的縮排（取第一個成員所在行的前導空白）。
fn indent_of(text: &str, m: &Member) -> String {
    let line_start = text[..m.key_start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    text[line_start..m.key_start].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// 切換目標的 `archived`。`service_id` 為 `None` 時切群組本身。回傳改好的整份文字。
/// 文字本身若不是合法 JSON 會先被 serde 擋下（不會半改）。
pub fn set_archived(text: &str, group_name: &str, service_id: Option<&str>, archived: bool) -> Result<String, String> {
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| format!("services.json 解析失敗: {e}"))?;
    let root = skip_ws(text.as_bytes(), 0);
    let group = find_in_array(text, root, "groups", "name", group_name)?
        .ok_or_else(|| format!("找不到群組「{group_name}」"))?;
    let target = match service_id {
        None => group,
        Some(id) => find_in_array(text, group, "services", "id", id)?
            .ok_or_else(|| format!("群組「{group_name}」裡找不到服務 id「{id}」"))?,
    };
    let ms = members(text, target)?;
    let existing = ms.iter().find(|m| m.key == "archived");
    let mut out = String::with_capacity(text.len() + 32);
    match (existing, archived) {
        (Some(m), true) => {
            out.push_str(&text[..m.value_start]);
            out.push_str("true");
            out.push_str(&text[m.value_end..]);
        }
        (Some(m), false) => {
            // 移除整個成員：優先連同前面的逗號（與其後的空白）一起刪；若是第一個成員則刪到下一個鍵之前
            let idx = ms.iter().position(|x| x == m).unwrap();
            let (cut_start, cut_end) = if idx > 0 {
                let prev_end = ms[idx - 1].value_end;
                let comma = text[prev_end..m.key_start].find(',').map(|p| prev_end + p).unwrap_or(prev_end);
                (comma, m.value_end)
            } else if ms.len() > 1 {
                (m.key_start, ms[1].key_start)
            } else {
                // 唯一成員：把 `{ "archived": true }` 收成 `{}`（實務上群組／服務至少還有 name／id）
                (target + 1, m.value_end)
            };
            out.push_str(&text[..cut_start]);
            out.push_str(&text[cut_end..]);
        }
        (None, true) => {
            let anchor = ms.iter().find(|m| m.key == "name").or_else(|| ms.iter().find(|m| m.key == "id")).or_else(|| ms.first());
            let Some(anchor) = anchor else { return Err("目標物件沒有任何成員".into()); };
            let indent = indent_of(text, anchor);
            out.push_str(&text[..anchor.value_end]);
            out.push(',');
            out.push_str(newline_of(text));
            out.push_str(&indent);
            out.push_str("\"archived\": true");
            out.push_str(&text[anchor.value_end..]);
        }
        (None, false) => return Ok(text.to_string()),
    }
    // 改完必須仍是合法 JSON，且只有目標的 archived 不同
    serde_json::from_str::<serde_json::Value>(&out).map_err(|e| format!("改寫後 JSON 不合法（未寫入）: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "_readme": "說明文字 {with:braces} \"quoted\"",
  "appName": "Workspace Hub",
  "groups": [
    {
      "name": "交易",
      "services": [
        {
          "id": "a",
          "name": "A 服務",
          "stop": { "type": "kill", "pattern": "x\\\\y", "processName": "python.exe" }
        },
        { "id": "b", "name": "B", "kind": "info" }
      ]
    },
    {
      "name": "舊",
      "archived": true,
      "services": [
        { "id": "c", "name": "C", "archived": false }
      ]
    }
  ]
}
"#;

    fn parsed(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn archive_service_inserts_after_name_and_keeps_everything_else() {
        let out = set_archived(SAMPLE, "交易", Some("a"), true).unwrap();
        assert!(out.contains("          \"name\": \"A 服務\",\n          \"archived\": true,\n          \"stop\": { \"type\": \"kill\""));
        let mut expect = parsed(SAMPLE);
        expect["groups"][0]["services"][0]["archived"] = serde_json::json!(true);
        assert_eq!(parsed(&out), expect);
        // 單行物件與 _readme 原樣
        assert!(out.contains(r#""stop": { "type": "kill", "pattern": "x\\\\y", "processName": "python.exe" }"#));
        assert!(out.contains(r#""_readme": "說明文字 {with:braces} \"quoted\"""#));
    }

    #[test]
    fn archive_then_unarchive_roundtrips_to_identical_text() {
        let archived = set_archived(SAMPLE, "交易", Some("a"), true).unwrap();
        let back = set_archived(&archived, "交易", Some("a"), false).unwrap();
        assert_eq!(back, SAMPLE);
        // 單行服務卡也一樣
        let archived = set_archived(SAMPLE, "交易", Some("b"), true).unwrap();
        assert!(archived.contains(r#"{ "id": "b", "name": "B",
        "archived": true, "kind": "info" }"#));
        assert_eq!(set_archived(&archived, "交易", Some("b"), false).unwrap(), SAMPLE);
    }

    #[test]
    fn group_toggle_only_changes_group_flag() {
        let out = set_archived(SAMPLE, "舊", None, false).unwrap();
        let mut expect = parsed(SAMPLE);
        expect["groups"][1].as_object_mut().unwrap().remove("archived");
        assert_eq!(parsed(&out), expect);
        assert!(out.contains("      \"name\": \"舊\",\n      \"services\": ["));
        let again = set_archived(&out, "舊", None, true).unwrap();
        assert_eq!(again, SAMPLE);
        // 群組封存時不碰群組內服務的 archived
        let out = set_archived(SAMPLE, "交易", None, true).unwrap();
        assert_eq!(parsed(&out)["groups"][0]["services"], parsed(SAMPLE)["groups"][0]["services"]);
    }

    #[test]
    fn existing_false_becomes_true_in_place() {
        let out = set_archived(SAMPLE, "舊", Some("c"), true).unwrap();
        assert!(out.contains(r#"{ "id": "c", "name": "C", "archived": true }"#));
        assert_eq!(out.len(), SAMPLE.len() - 1);
    }

    #[test]
    fn unarchive_without_flag_is_noop_and_unknown_targets_error() {
        assert_eq!(set_archived(SAMPLE, "交易", Some("a"), false).unwrap(), SAMPLE);
        assert!(set_archived(SAMPLE, "沒有", None, true).unwrap_err().contains("找不到群組"));
        assert!(set_archived(SAMPLE, "交易", Some("zzz"), true).unwrap_err().contains("找不到服務"));
        assert!(set_archived("{ not json", "交易", None, true).is_err());
    }

    #[test]
    fn crlf_files_get_crlf_insertions() {
        let crlf = SAMPLE.replace('\n', "\r\n");
        let out = set_archived(&crlf, "交易", Some("a"), true).unwrap();
        assert!(out.contains("\"name\": \"A 服務\",\r\n          \"archived\": true,\r\n"));
        assert_eq!(set_archived(&out, "交易", Some("a"), false).unwrap(), crlf);
    }

    #[test]
    fn real_services_json_roundtrip_if_present() {
        let Ok(text) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../services.json")) else { return; };
        let out = set_archived(&text, "開發中專案", Some("money-manager"), true).unwrap();
        assert_ne!(out, text);
        assert_eq!(set_archived(&out, "開發中專案", Some("money-manager"), false).unwrap(), text);
    }
}
