# Workspace Hub

Config-driven desktop launcher for local services and dev projects (Tauri 2, Windows).
設定驅動的本機桌面程式：把 workspace 裡所有本機服務（交易工具、dev server、排程任務、純說明連結）
集中成一張控制台式表格，點名稱就開頁面（需要 token 的從檔案自動帶入），不必記 localhost 端口。

## What it is

- **一個 JSON 管一切**：`services.json` 決定群組、服務、健康探測、啟停命令、開啟連結。加服務＝加一段 JSON，不用重編譯。
- **四種列型態**：一般服務（探 `healthUrl`、啟／停、開頁面）、`status`（Windows 工作排程器任務的上次／下次執行＋log 尾行）、
  `info`（純說明，可帶外部連結）、封存（隱藏、不探、不啟停；可在 UI 直接切）。
- **Token 不進前端**：`open` / `healthUrl` 的 `{file:<路徑>}` 樣板在 Rust 端讀檔展開，WebView 看不到 token 內容。
- **只有一個視窗**：single-instance、關閉縮到系統匣、AppUserModelID 讓工作列釘選與執行中視窗合併成一個圖示。
- 本機安裝，不是可攜版：路徑寫死在 `E:\Kyle\Workspace\...`（`src-tauri/src/lib.rs` 的 `SERVICES_PATH` / `TODOS_PATH`，
  可用環境變數 `HUB_SERVICES` 覆蓋設定路徑），依賴各專案與 WebView2。

![Workspace Hub 截圖](screenshots/hub.png)
<!-- TODO: 補 screenshots/hub.png（主視窗：交易群組展開、一列綠燈一列排程狀態） -->

## 版面

2026-10-07 起改成控制台式表格清單：每個群組一個區段標題，底下一列一個服務
（狀態點＋字｜名稱＋說明｜來源（artifact／port／排程）｜等寬字的上次／下次｜文字型操作）；
整頁一個深藍底＋琥珀強調色（與圖示同色），沒有卡片框、圓角方塊或陰影。文中「卡片」指的是 `services.json` 的項目型態。

關閉視窗會縮到系統匣；再次開捷徑會回到同一個程式（single-instance）。系統匣「退出」只退出桌面程式，
服務與排程任務繼續運作。

## 建置與捷徑

需要 Rust MSVC 工具鏈（`rust-toolchain.toml` 釘 `stable-x86_64-pc-windows-msvc`）、Visual Studio C++ 工具組、WebView2。
**改 `ui/` 或 `src-tauri/` 之後，必須跑 release build：**

```powershell
cd E:\Kyle\Workspace\workspace-hub
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-agent-desktop.ps1 -InstallShortcuts
```

腳本在 `.tmp\hub-desktop-target` 建 release，交付到**固定路徑** `E:\Kyle\Workspace\workspace-hub\runtime\desktop\current\agent-hub.exe`
（桌面／開始功能表／已釘選的工作列捷徑都指它），同時留一份版本副本 `runtime\desktop\<日期時間>\agent-hub.exe`
＋ `delivery.json`（exe/來源雜湊、`appName`、AppUserModelID、路徑；`current\` 也放一份）。`runtime/` 整個 gitignored，可從原始碼重建。
捷徑名稱＝`appName`。覆蓋 current 前，腳本只退出「正在從 current 路徑執行」的 Hub 程序（精確比對 exe 路徑，不按名稱批次殺）；
從別的路徑跑著的實例不碰，要自己從系統匣退出，否則 single-instance 會把新捷徑導回舊實例。
建完請從 current 路徑啟動。腳本檔本身存成 UTF-8 **含 BOM**（Windows PowerShell 5.1 讀無 BOM 檔會當 CP950，
中文註解會吞掉後面的引號）。

單元測試：`cargo test --manifest-path src-tauri\Cargo.toml`（含真的呼叫一次 schtasks 的編碼測試，
以及 `archive.rs` 對真實 `services.json` 的封存往返測試）。

### 工作列只出現一個圖示（AppUserModelID）

Windows 以 AppUserModelID（沒有就用 exe 路徑）決定「釘選的捷徑」和「執行中的視窗」要不要合併成同一個工作列按鈕。
三處同一個 ID `tw.kyle.agenthub`：`src-tauri/tauri.conf.json` 的 `identifier`、`src-tauri/src/main.rs` 在建視窗前呼叫
`SetCurrentProcessExplicitAppUserModelID`（Tauri 2.11 自己不會設）、build 腳本把它寫進每個 .lnk 的
`System.AppUserModel.ID`（IShellLinkW＋IPropertyStore，WScript.Shell 寫不到；驗證走 Shell.Application `ExtendedProperty`）。
腳本若在 `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\` 找到同名捷徑，
也只改它的目標／圖示／ID 指向 current（不碰登錄、不必重釘）。腳本會檢查三處 ID 一致，改 ID 要三處一起改。
`src-tauri\target\release` 只是 cargo 產物，不要對它建捷徑或釘選。

圖示：`scripts/make_icon.py`（Pillow）產生 `src-tauri/icons/` 全部尺寸（含多尺寸 icon.ico），改圖示＝改腳本→重跑→重建。

### 改名

`services.json` 的 `appName`（預設 `Workspace Hub`）決定視窗標題、系統匣提示與選單、捷徑檔名（build 腳本讀同一個欄位）。
**exe 檔名（agent-hub.exe）、single-instance 鎖與設定路徑不跟著變**，改名只要改 JSON 再跑一次 build 腳本裝新捷徑；舊捷徑不會自動刪。

### 為什麼漏掉重建會出現詭異錯誤

UI（`ui/`）是**打包進 binary** 的（`tauri.conf.json` 的 `frontendDist: "../ui"`），但 `services.json` 是**執行時從磁碟讀**的。
只改檔沒重建 release 時會出現「新設定 × 舊 UI」的錯配，錯誤訊息通常指不到真正原因
（2026-07-30 實例：新欄位讓舊 UI 整頁卡在「設定載入失敗: invalid args」）。
**驗收 release 真的換掉了**：比對 `runtime\desktop\current\agent-hub.exe` 的 LastWriteTime 與你改動的時間，不要只看「cargo 說 Finished」。

## services.json 欄位

### 頂層與群組

| 欄位 | 層級 | 說明 |
|---|---|---|
| `_readme` | 頂層 | 給人看的說明，程式忽略 |
| `appName` | 頂層 | 視窗標題／系統匣／捷徑名；空值＝`Workspace Hub` |
| `groups[]` | 頂層 | 群組陣列，顯示順序＝陣列順序 |
| `name` | 群組 | 區段標題 |
| `archived` | 群組 | `true`＝整組封存（預設隱藏、不探健康、無啟停） |
| `services[]` | 群組 | 服務陣列 |

### 服務

| 欄位 | 適用 kind | 說明 |
|---|---|---|
| `id` | 全部 | 唯一識別，前端狀態與 Rust 端 spawn 表的鍵 |
| `name` / `desc` | 全部 | 名稱（可點）／說明 |
| `kind` | — | 不填＝一般服務；`status`＝唯讀狀態列；`info`＝純說明列 |
| `archived` | 全部 | `true`＝封存此列（UI 的「封存」按鈕也改這個欄位） |
| `source` | 全部 | 「來源」欄要顯示的字；不填則自動推（artifact／port／排程／說明） |
| `start` / `cwd` | 一般 | 啟動命令與工作目錄（會寫本地資料的服務 `cwd` 必填） |
| `healthUrl` | 一般 | 健康探測，**要用真實 API 路由**（端口有回應 ≠ 服務健康） |
| `open` | 一般、info | 點名稱開的網址；支援樣板 |
| `external` | info | `true`＝用系統瀏覽器開 `open`（例如 claude.ai artifact） |
| `stop` | 一般 | `{type:"http", url}` 或 `{type:"kill", pattern, processName}`；kill **一定要設 `processName`**（2026-07-30 誤殺教訓） |
| `taskName` | status | Windows 工作排程器任務名，走 `schtasks /query /fo csv /v` 讀上次／下次執行與結果 |
| `statusFiles.logFile` | status | 檔案最後一行：有 `[時間]` 前綴取「最近檢查」，否則顯示最後一行（截 160 字） |
| `statusFiles.alertFile` | status | 檔案存在且非空＝紅燈＋內容（看門狗型） |
| `statusFiles.resultFile` | status | JSON `{status, checkedAt, message}`（`runOnHubStart` 寫入） |
| `runOnHubStart` | status | Hub 啟動時的一次性動作：等 `dependsOnHealthUrl` 健康後執行 `command`，`stateFile` 記日期每日一次，結果寫 `resultFile` |
| `note` | info | 補充說明 |

排程結果碼：0 成功、267009 執行中、267011 尚未執行、其餘失敗。`taskName` 刻意不用 WMI（本機 WMI 曾卡死）。

### 字串樣板

`open`、`healthUrl`、`statusFiles.*` 可用樣板，一律在 Rust 端展開（`src-tauri/src/template.rs`，前端拿不到 token 內容）：

| 樣板 | 意義 | 例 |
|---|---|---|
| `{file:<路徑>}` | 讀檔內容去頭尾空白代入；讀不到或空檔＝「尚未啟動」（點開時顯示、探健康視同未啟動） | `http://127.0.0.1:4327/?token={file:E:\Kyle\Workspace\tw-stock-pullback\.session-token}` |
| `{date:<格式>}` | 本機日期，.NET 風格 `yyyy MM dd HH mm ss` | `...\logs\{date:yyyyMMdd}-record.log` |

### 封存（archived）

群組或服務加 `"archived": true`：預設隱藏，右上「顯示封存」切換後只顯示名稱與說明——不探健康、無啟停、
不開連結、不跑 `runOnHubStart`。

也可以在 UI 直接切：每列右側「封存」／封存列的「取消封存」，群組標題列右側「封存群組」／「取消封存」。
按下去會出現同位置的 inline 確認列（不用 window.confirm，那會卡住 WebView），確定後由 Rust 端
（`src-tauri/src/archive.rs`）以**純文字方式**改寫 `services.json`：只動目標的 `archived` 成員，
縮排、單行物件、`_readme`、鍵順序與行尾全部原樣，寫入前先存 `services.json.bak`（只保留最近一次，gitignored）。
群組已封存時服務列不給個別按鈕，由群組層級處理。`services.json` 仍可手改：改完按「重新整理」會重新從磁碟讀（或重開 Hub）。

### 群組收合

點群組標題列（箭頭／名稱）收合或展開；收合時標題列右側顯示該群組的健康摘要（例如「3 運行中・1 停止・1 異常」，
資訊列不計）。收合狀態存在 WebView2 的 `localStorage`（鍵 `hub.collapsedGroups`，以群組名稱記）；群組改名＝視為新群組。

## 目錄

| 路徑 | 內容 |
|---|---|
| `services.json` | 設定（唯一需要手改的檔） |
| `ui/` | 前端（打包進 exe） |
| `src-tauri/src/` | `lib.rs` 指令與啟停、`template.rs` 樣板、`schtasks.rs` 排程狀態、`archive.rs` 封存改寫、`main.rs` AUMID＋入口 |
| `scripts/build-agent-desktop.ps1` | release 建置＋交付＋捷徑 |
| `scripts/make_icon.py` | 產生圖示 |
| `data/`（gitignored） | `todos.json` 個人待辦 |
| `runtime/desktop/`（gitignored） | 交付版 |

## 其他容易踩的點

- `stop.type: "kill"` 只對完整命令列做 regex 會誤殺任何「提到」該檔名的程序，實測命中過 3 個無辜的 bash shell（2026-07-30 教訓）；
  pattern 寫得越具體越好，並一併設 `processName`。
- `stop.type: "kill"` 的命令列比對走 `Get-CimInstance`（WMI）；WMI 卡住時停止會逾時，Hub 自己啟動的服務仍先按 PID 殺樹。
- Hub 管不到**別的 logon session（排程任務 S4U）啟動的程序**：那類程序讀不到命令列、非提權也殺不掉。排程任務一律用 `status` 列只看不碰。
- 2026-10-07 之前每次交付換一個日期路徑，釘選的捷徑指舊 exe，點開後工作列就多出第二個圖示；現在固定 current 路徑＋AUMID 解決。

## 歷史

本專案原為 `agent-harness` repo 的 `hub/` 子目錄（Agent Hub），2026-10-08 以 `git subtree split` 拆成獨立 repo，commit 歷史保留。
agent-harness 控制台群組已於 2026-10-07 封存；早期 P0 交付紀錄留在 agent-harness 的 `docs/notes/agent-harness-p0-delivery.md`。

## License

MIT — see [LICENSE](LICENSE).
