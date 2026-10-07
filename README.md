# Workspace Hub（Tauri 本機桌面程式）

整個 workspace 的統一入口：所有本機服務（交易工具、開發中專案的 dev server、排程任務）集中成卡片，
點名稱就開頁面（需要 token 的會自動從檔案帶入），不必記 localhost 端口。設定全在 `services.json`，
加服務＝加一段 JSON，不用重編譯。

2026-10-07 起 agent-harness 控制台已過期、群組封存；Hub 不再於啟動時自動拉起 4317/4320。
歷史 P0 交付紀錄見 [docs/notes/agent-harness-p0-delivery.md](../docs/notes/agent-harness-p0-delivery.md)。

關閉視窗會縮到系統匣；再次開捷徑會回到同一個程式（single-instance）。系統匣「退出」只退出桌面程式，
服務與排程任務繼續運作。這是本機安裝，不是可攜版：依賴 `E:\Kyle\Workspace` 下各專案與 WebView2。

## 改名

`services.json` 的 `appName`（預設 `Workspace Hub`）決定視窗標題、系統匣提示與選單、捷徑檔名
（build 腳本讀同一個欄位）。**exe 檔名（agent-hub.exe）、single-instance 鎖與設定路徑不跟著變**，
所以改名只要改 JSON 再跑一次 build 腳本裝新捷徑；舊捷徑不會自動刪。

## 建置與捷徑

**改 `ui/` 或 `src-tauri/` 之後，必須跑 release build：**

```powershell
cd E:\Kyle\Workspace\agent-harness
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-agent-desktop.ps1 -InstallShortcuts
```

腳本在 `.tmp/hub-desktop-target` 建 release，交付至 `runtime/desktop/<日期時間>/agent-hub.exe`，並寫入
exe/來源雜湊、`appName` 與路徑的 `delivery.json`；捷徑裝到桌面與開始功能表，名稱＝`appName`。
腳本不停止任何程式；更新時若舊程式仍在跑，single-instance 會使新捷徑回到舊實例，需先從系統匣退出
舊桌面實例（只退出 Hub 自己的程序，不按名稱批次殺）再開新版。

建置需要 Rust MSVC 與 Visual Studio C++ 工具組；單元測試 `cargo test --manifest-path hub\src-tauri\Cargo.toml`
（含真的呼叫一次 schtasks 的編碼測試）。Tauri 使用系統 WebView2；要製作跨機安裝包還需另處理各專案依賴與
資料路徑，不能把目前 exe 當成完整安裝程式。[官方前置需求](https://v2.tauri.app/start/prerequisites/)、
[Windows 封裝文件](https://v2.tauri.app/distribute/windows-installer/)。

### 為什麼漏掉重建會出現詭異錯誤

UI（`ui/`）是**打包進 binary** 的（`tauri.conf.json` 的 `frontendDist: "../ui"`），但 `services.json`
是**執行時從磁碟讀**的。只改檔沒重建 release 時會出現「新設定 × 舊 UI」的錯配，錯誤訊息通常指不到真正原因
（2026-07-30 實例：新欄位讓舊 UI 整頁卡在「設定載入失敗: invalid args」）。
**驗收 release 真的換掉了**：比對 exe 的 LastWriteTime 與你改動的時間，不要只看「cargo 說 Finished」。

## services.json 欄位

頂層：`appName`、`groups[]`。群組：`name`、`archived`、`services[]`。

### 卡片型態

| kind | 行為 | 需要的欄位 |
|---|---|---|
| （不填） | 一般服務：探 `healthUrl` 顯示燈號，有啟停按鈕，點名稱開 `open` | `start` / `cwd` / `healthUrl` / `stop`；`open` 選填 |
| `status` | 唯讀狀態卡，無按鈕。排程任務、沒有 HTTP 端點的背景程序 | `taskName` 和／或 `statusFiles`（下表） |
| `info` | 純說明卡（設計期專案、不由 Hub 啟動的東西） | `desc`、`note` 選填 |

`status` 卡的資料來源可疊加：

| 欄位 | 來源 | 顯示 |
|---|---|---|
| `taskName` | `schtasks /query /tn <名稱> /fo csv /v`（刻意不用 WMI；本機 WMI 曾卡死） | 上次執行時間／結果（0 成功、267009 執行中、267011 尚未執行、其餘失敗）／下次執行時間 |
| `statusFiles.logFile` | 檔案最後一行 | 有 `[時間]` 前綴取「最近檢查」，否則顯示最後一行（截 160 字） |
| `statusFiles.alertFile` | 檔案存在且非空＝紅燈＋內容 | 看門狗型 |
| `statusFiles.resultFile` | JSON `{status, checkedAt, message}` | 動作結果型（`runOnHubStart` 寫入） |

`kind:"status"` 再加 `runOnHubStart` 可宣告「Hub 啟動時的一次性動作」：等 `dependsOnHealthUrl` 健康後執行
`command`，靠 `stateFile` 記錄的日期做到每日只跑一次，結果寫進 `resultFile`。封存的項目不會執行。

### 封存（archived）

群組或服務加 `"archived": true`：預設隱藏，右上「顯示封存」切換後只顯示名稱與說明——不探健康、無啟停、
不開連結、不跑 `runOnHubStart`。agent-harness 群組（控制台、看門狗）2026-10-07 起封存。

### 字串樣板

`open`、`healthUrl`、`statusFiles.*` 可用樣板，一律在 Rust 端展開（前端拿不到 token 內容）：

| 樣板 | 意義 | 例 |
|---|---|---|
| `{file:<路徑>}` | 讀檔內容去頭尾空白代入；讀不到或空檔＝「尚未啟動」（點開時顯示、探健康視同未啟動） | `http://127.0.0.1:4327/?token={file:E:\Kyle\Workspace\tw-stock-pullback\.session-token}` |
| `{date:<格式>}` | 本機日期，.NET 風格 `yyyy MM dd HH mm ss` | `...\logs\{date:yyyyMMdd}-record.log` |

## 目前登記的卡片（2026-10-07）

- 交易：台股紀律檢查（4327，`/api/health` 不需 token）、交易駕駛艙（8787，固定 `--broker fake`，首頁 200 當健康）、
  五張排程狀態卡（群益／富邦錄製器、永豐回補、每晚資料排程、庫存同步，log 按 `{date:yyyyMMdd}` 命名）
- 開發中專案：money_manager（vite 5173）、graph_rag（Django 8000 `/healthz`，用專案自己的 .venv）、geyser／daily-speak 為 info 卡
- agent-harness：封存

## 其他容易踩的點

- `healthUrl` 要用**真實 API 路由**，端口有回應 ≠ 服務健康（2026-07-06 教訓）；駕駛艙目前沒有不需 token 的 API，暫用首頁
- `cwd` 對會寫本地資料的服務必填
- `stop.type: "kill"` 一定要一併設 `stop.processName`（例如 `python.exe`）。只對完整命令列做 regex 會誤殺任何
  「提到」該檔名的程序，實測命中過 3 個無辜的 bash shell（2026-07-30 教訓）。pattern 寫得越具體越好。
- `stop.type: "kill"` 的命令列比對仍走 `Get-CimInstance`（WMI）；WMI 卡住時停止會逾時，Hub 自己啟動的服務仍先按 PID 殺樹
- Hub 管不到**別的 logon session（排程任務 S4U）啟動的程序**：那類程序讀不到命令列、非提權也殺不掉。排程任務
  一律用 `status` 卡只看不碰
- 紀律工具的 `/api/health` 是 2026-10-07 加的；在那之前啟動的實例要重啟後燈號才會亮
