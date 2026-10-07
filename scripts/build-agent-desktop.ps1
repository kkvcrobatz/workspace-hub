param(
    [switch]$SkipBuild,
    [switch]$InstallShortcuts
)
$ErrorActionPreference = 'Stop'
function Get-DesktopSha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $sha.Dispose() }
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$hubRoot = $repoRoot
$targetDir = Join-Path $repoRoot '.tmp\hub-desktop-target'
$sourceExe = Join-Path $targetDir 'release\agent-hub.exe'
$deliveryRoot = Join-Path $repoRoot 'runtime\desktop'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
# 捷徑名稱跟著 services.json 的 appName（視窗標題／系統匣也用它）；exe 檔名與單實例鎖不變。
$servicesJson = Get-Content -LiteralPath (Join-Path $hubRoot 'services.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$appName = if ($servicesJson.appName -and $servicesJson.appName.Trim()) { $servicesJson.appName.Trim() } else { 'Workspace Hub' }
if ($appName -match '[\\/:*?"<>|]') { throw "appName 含捷徑檔名不允許的字元: $appName" }
# AppUserModelID：與 tauri.conf.json 的 identifier、main.rs 的 APP_USER_MODEL_ID 一致；寫進捷徑讓工作列把釘選與執行中視窗合併成一個圖示。
$tauriConf = Get-Content -LiteralPath (Join-Path $hubRoot 'src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$appUserModelId = [string]$tauriConf.identifier
if (-not $appUserModelId) { throw 'tauri.conf.json 缺 identifier（AppUserModelID）' }
$mainRs = Get-Content -LiteralPath (Join-Path $hubRoot 'src-tauri\src\main.rs') -Raw -Encoding UTF8
if ($mainRs -notmatch ('APP_USER_MODEL_ID: &str = "' + [regex]::Escape($appUserModelId) + '"')) { throw "main.rs 的 APP_USER_MODEL_ID 與 tauri.conf.json identifier（$appUserModelId）不一致" }

# 交付：固定路徑 runtime\desktop\current\agent-hub.exe（捷徑永遠指它，釘選不會因版本換路徑而分成兩個圖示）
# ＋ runtime\desktop\<日期時間>\ 版本副本與 delivery.json。
$versionDir = Join-Path $deliveryRoot $stamp
$currentDir = Join-Path $deliveryRoot 'current'
$exe = Join-Path $currentDir 'agent-hub.exe'

# Build into an isolated target.
if (-not $SkipBuild) {
    Push-Location $hubRoot
    try {
        & cargo build --manifest-path src-tauri/Cargo.toml --release --locked --target-dir $targetDir
        if ($LASTEXITCODE -ne 0) { throw 'Desktop release build failed' }
    } finally { Pop-Location }
}
if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) { throw "Release executable missing: $sourceExe" }
if (Test-Path -LiteralPath $versionDir) { throw "Delivery already exists: $versionDir" }
New-Item -ItemType Directory -Path $versionDir -Force | Out-Null
New-Item -ItemType Directory -Path $currentDir -Force | Out-Null

# 覆蓋 current 之前，只退出「正在從 current 路徑執行」的 Hub 程序（精確比對 exe 路徑，不按名稱批次殺）。
$stopped = @()
foreach ($proc in @(Get-Process -Name 'agent-hub' -ErrorAction SilentlyContinue)) {
    $procPath = try { $proc.Path } catch { $null }
    if ($procPath -and ([IO.Path]::GetFullPath($procPath) -ieq [IO.Path]::GetFullPath($exe))) {
        Write-Host "Stopping running Hub from current path (PID $($proc.Id))"
        Stop-Process -Id $proc.Id -Force -Confirm:$false
        $proc.WaitForExit(10000) | Out-Null
        $stopped += $proc.Id
    }
}
$sourceHash = Get-DesktopSha256 $sourceExe
$versionExe = Join-Path $versionDir 'agent-hub.exe'
Copy-Item -LiteralPath $sourceExe -Destination $versionExe
Copy-Item -LiteralPath $sourceExe -Destination $exe -Force
foreach ($copy in @($versionExe, $exe)) { if ((Get-DesktopSha256 $copy) -ne $sourceHash) { throw "Executable copy verification failed: $copy" } }
$manifest = [ordered]@{
    builtAt = (Get-Date).ToUniversalTime().ToString('o')
    executable = $exe
    versionCopy = $versionExe
    sha256 = $sourceHash
    repo = $repoRoot
    mode = 'local-workspace-dependent'
    appName = $appName
    appUserModelId = $appUserModelId
    servicesConfig = (Join-Path $hubRoot 'services.json')
    stoppedProcesses = $stopped
    requirements = @('Existing workspace projects referenced by services.json', 'Microsoft Edge WebView2 Runtime')
    shortcuts = @()
    sourceHashes = @()
}
foreach ($sourceRoot in @((Join-Path $hubRoot 'ui'), (Join-Path $hubRoot 'src-tauri\src'))) {
    foreach ($source in Get-ChildItem -LiteralPath $sourceRoot -File -Recurse) {
        $manifest.sourceHashes += @{ path = $source.FullName.Substring($repoRoot.Length + 1); sha256 = (Get-DesktopSha256 $source.FullName) }
    }
}
foreach ($relative in @('src-tauri\Cargo.toml', 'src-tauri\Cargo.lock', 'src-tauri\tauri.conf.json', 'src-tauri\build.rs')) {
    $path = Join-Path $hubRoot $relative
    $manifest.sourceHashes += @{ path = $relative; sha256 = (Get-DesktopSha256 $path) }
}

# 把 System.AppUserModel.ID 寫進 .lnk（IShellLinkW + IPropertyStore；WScript.Shell 寫不到這個屬性）。
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
namespace HubShortcut {
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    public struct PropertyKey { public Guid fmtid; public uint pid; public PropertyKey(Guid f, uint p) { fmtid = f; pid = p; } }
    [StructLayout(LayoutKind.Explicit)]
    public struct PropVariant {
        [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr ptr;
        public static PropVariant FromString(string s) { var v = new PropVariant(); v.vt = 31; v.ptr = Marshal.StringToCoTaskMemUni(s); return v; }
        public void Clear() { if (ptr != IntPtr.Zero) { Marshal.FreeCoTaskMem(ptr); ptr = IntPtr.Zero; } vt = 0; }
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IPropertyStore {
        void GetCount(out uint count); void GetAt(uint i, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] public class ShellLink { }
    public static class Aumid {
        static readonly PropertyKey Key = new PropertyKey(new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 5);
        public static void Write(string lnk, string id) {
            object link = new ShellLink();
            var file = (IPersistFile)link;
            file.Load(lnk, 2 /* STGM_READWRITE */);
            var store = (IPropertyStore)link;
            var v = PropVariant.FromString(id);
            try { var k = Key; store.SetValue(ref k, ref v); store.Commit(); } finally { v.Clear(); }
            file.Save(lnk, true);
        }
    }
}
'@ -ErrorAction Stop

function Install-HubShortcut([string]$LinkPath) {
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($LinkPath)
    $link.TargetPath = $exe
    $link.WorkingDirectory = $hubRoot
    $link.IconLocation = "$exe,0"
    $link.Description = "$appName — workspace 統一入口（本機桌面程式）"
    $link.Save()
    [HubShortcut.Aumid]::Write($LinkPath, $appUserModelId)
    $readBack = $shell.CreateShortcut($LinkPath)
    if ($readBack.TargetPath -ne $exe -or $readBack.WorkingDirectory -ne $hubRoot) { throw "Shortcut verification failed: $LinkPath" }
    $folderItem = (New-Object -ComObject Shell.Application).NameSpace((Split-Path -Parent $LinkPath)).ParseName((Split-Path -Leaf $LinkPath))
    if ([string]$folderItem.ExtendedProperty('System.AppUserModel.ID') -ne $appUserModelId) { throw "Shortcut AppUserModelID verification failed: $LinkPath" }
}

if ($InstallShortcuts) {
    foreach ($folder in @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'))) {
        if (-not $folder -or -not (Test-Path -LiteralPath $folder -PathType Container)) { throw 'Windows shortcut folder unavailable' }
        $linkPath = Join-Path $folder "$appName.lnk"
        if (Test-Path -LiteralPath $linkPath) {
            $label = if ($folder -eq [Environment]::GetFolderPath('Desktop')) { 'desktop' } else { 'start-menu' }
            Copy-Item -LiteralPath $linkPath -Destination (Join-Path $versionDir "previous-$label.lnk")
        }
        Install-HubShortcut $linkPath
        $manifest.shortcuts += $linkPath
    }
    # 使用者釘在工作列的捷徑：只改目標／圖示／AUMID 指向 current（不碰登錄、不重釘）。沒有就略過。
    $pinned = Join-Path $env:APPDATA "Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\$appName.lnk"
    if (Test-Path -LiteralPath $pinned -PathType Leaf) {
        Copy-Item -LiteralPath $pinned -Destination (Join-Path $versionDir 'previous-taskbar.lnk')
        Install-HubShortcut $pinned
        $manifest.shortcuts += $pinned
    }
}
$manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $versionDir 'delivery.json') -Encoding UTF8
Copy-Item -LiteralPath (Join-Path $versionDir 'delivery.json') -Destination (Join-Path $currentDir 'delivery.json') -Force
$manifest | ConvertTo-Json -Depth 4
Write-Host "Delivered to $exe (version copy: $versionDir). Shortcut name: $appName; AppUserModelID: $appUserModelId. Only a Hub running from the current path was stopped; start the new build from $exe."
