# BMCBL 原生窗口渲染探针。
#
# 对正在运行的主窗口执行“类真人”操作，并收集同一时间段的 GPUI Trace 片段、
# 前后截图与统计摘要。不会启动第二个实例，也不会修改配置。
#
# 用法：
#   pwsh -File scripts/ui/manage-render-probe.ps1 -ResizeIterations 240
#   pwsh -File scripts/ui/manage-render-probe.ps1 -Clicks "0.33,0.137;0.40,0.137" -ResizeIterations 0
#
# Clicks 以窗口矩形为基准，按 0<a<1 的相对坐标给出，用分号分隔；脚本连续执行 8 轮。
# 输出目录包含 before.png、after.png、log-slice.txt 与 summary.json。
# 测量前请在设置中启用“GPUI / Nova 详细 Trace”（重启生效）。
param(
    [string]$ProcessName = 'BMCBL',
    [string]$WindowTitle = 'BMCBL',
    [int]$ResizeIterations = 180,
    [string]$Clicks = '',
    [int]$ClickIntervalMs = 55,
    [long]$Seed = 20261021,
    [string]$OutputDirectory = (Join-Path $env:TEMP 'bmcbl-ui-probe')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

if (-not ('BmcblUiProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class BmcblUiProbe {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, UIntPtr w, IntPtr l);
}
'@
}

function Find-AppWindow {
    param([string]$Name, [string]$Title)

    $target = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $target) { throw "BMCBL process not found: $Name" }

    $script:probeWindow = [IntPtr]::Zero
    $script:probeTargetId = $target.Id
    $callback = [BmcblUiProbe+EnumProc] {
        param($handle, $unused)
        $windowPid = 0
        [BmcblUiProbe]::GetWindowThreadProcessId($handle, [ref]$windowPid) | Out-Null
        if ($windowPid -ne $script:probeTargetId) { return $true }
        $builder = New-Object System.Text.StringBuilder 512
        [BmcblUiProbe]::GetWindowTextW($handle, $builder, 512) | Out-Null
        if ($builder.ToString() -eq $Title) {
            $script:probeWindow = $handle
            return $false
        }
        return $true
    }
    [BmcblUiProbe]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
    if ($script:probeWindow -eq [IntPtr]::Zero) { throw "window not found: $Title" }
    return $script:probeWindow
}

function Get-WindowRect {
    param([IntPtr]$Handle)
    $rect = New-Object BmcblUiProbe+RECT
    [BmcblUiProbe]::GetWindowRect($Handle, [ref]$rect) | Out-Null
    return $rect
}

function Save-WindowScreenshot {
    param([IntPtr]$Handle, [string]$Path)
    $rect = Get-WindowRect $Handle
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -le 0 -or $height -le 0) { return $null }
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    $captured = [BmcblUiProbe]::PrintWindow($Handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $graphics.Dispose()
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    return [pscustomobject]@{ Path = $Path; Width = $width; Height = $height; Captured = [bool]$captured }
}

function Read-LogSlice {
    param([string]$Path, [long]$Offset)
    $share = [System.IO.FileShare]::ReadWrite
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, $share)
    try {
        if ($stream.Length -le $Offset) { return '' }
        $stream.Seek($Offset, [System.IO.SeekOrigin]::Begin) | Out-Null
        $reader = New-Object System.IO.StreamReader($stream)
        return $reader.ReadToEnd()
    } finally {
        $stream.Dispose()
    }
}

function Invoke-Click {
    param([IntPtr]$Handle, [int]$X, [int]$Y)
    [BmcblUiProbe]::SetCursorPos($X, $Y) | Out-Null
    Start-Sleep -Milliseconds 12
    if ($script:probeRealInput) {
        [BmcblUiProbe]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 10
        [BmcblUiProbe]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
        return
    }
    # A background shell cannot always steal foreground focus. Post a client-
    # coordinate click directly to the HWND so the probe still exercises the UI.
    $point = New-Object BmcblUiProbe+POINT
    $point.X = $X
    $point.Y = $Y
    [BmcblUiProbe]::ScreenToClient($Handle, [ref]$point) | Out-Null
    $lparam = [IntPtr](($point.Y -shl 16) -bor ($point.X -band 0xFFFF))
    [BmcblUiProbe]::PostMessage($Handle, 0x0201, [UIntPtr]1, $lparam) | Out-Null
    Start-Sleep -Milliseconds 10
    [BmcblUiProbe]::PostMessage($Handle, 0x0202, [UIntPtr]0, $lparam) | Out-Null
}

function Invoke-ResizeBurst {
    param([IntPtr]$Handle, [int]$Iterations, [long]$RandomSeed)
    $random = New-Object System.Random $RandomSeed
    $origin = Get-WindowRect $Handle
    $baseWidth = $origin.Right - $origin.Left
    $baseHeight = $origin.Bottom - $origin.Top
    $flags = 0x0002 -bor 0x0004 -bor 0x0010
    for ($index = 0; $index -lt $Iterations; $index++) {
        $width = [Math]::Max(640, $baseWidth + $random.Next(-260, 520))
        $height = [Math]::Max(420, $baseHeight + $random.Next(-140, 380))
        [BmcblUiProbe]::SetWindowPos($Handle, [IntPtr]::Zero, 0, 0, $width, $height, $flags) | Out-Null
        $delay = if ($index % 60 -eq 0) { 40 } else { 6 }
        Start-Sleep -Milliseconds $delay
    }
    [BmcblUiProbe]::SetWindowPos($Handle, [IntPtr]::Zero, $origin.Left, $origin.Top, $baseWidth, $baseHeight, 0x0004 -bor 0x0010) | Out-Null
    Start-Sleep -Milliseconds 800
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$window = Find-AppWindow -Name $ProcessName -Title $WindowTitle
$logPath = Join-Path (Split-Path -Parent (Get-Process -Name $ProcessName | Select-Object -First 1).Path) 'BMCBL\logs\latest.log'
$logOffset = 0L
if (Test-Path $logPath) { $logOffset = (Get-Item $logPath).Length }

[BmcblUiProbe]::SetForegroundWindow($window) | Out-Null
[BmcblUiProbe]::BringWindowToTop($window) | Out-Null
Start-Sleep -Milliseconds 400
$script:probeRealInput = ([BmcblUiProbe]::GetForegroundWindow() -eq $window)

$before = Save-WindowScreenshot -Handle $window -Path (Join-Path $OutputDirectory 'before.png')

$startedAt = Get-Date
if ($ResizeIterations -gt 0) {
    Invoke-ResizeBurst -Handle $window -Iterations $ResizeIterations -RandomSeed $Seed
}

if ($Clicks -ne '') {
    $rect = Get-WindowRect $window
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    $points = $Clicks.Split(';') | Where-Object { $_.Trim() -ne '' }
    for ($round = 0; $round -lt 8; $round++) {
        foreach ($point in $points) {
            $parts = $point.Split(',')
            if ($parts.Count -ne 2) { continue }
            $x = $rect.Left + [int]([double]$parts[0] * $width)
            $y = $rect.Top + [int]([double]$parts[1] * $height)
            Invoke-Click -Handle $window -X $x -Y $y
            Start-Sleep -Milliseconds $ClickIntervalMs
        }
    }
    Start-Sleep -Milliseconds 1200
}

$after = Save-WindowScreenshot -Handle $window -Path (Join-Path $OutputDirectory 'after.png')
$finishedAt = Get-Date

$slice = ''
if (Test-Path $logPath) { $slice = Read-LogSlice -Path $logPath -Offset $logOffset }
$slicePath = Join-Path $OutputDirectory 'log-slice.txt'
Set-Content -LiteralPath $slicePath -Value $slice -Encoding utf8

$clickRounds = if ($Clicks -ne '') { 8 } else { 0 }
$resizeMismatches = 0
$lastNativeExtent = ''
foreach ($line in ($slice -split "`n")) {
    if ($line -match 'stage=native .*physical_px=([0-9]+x[0-9]+)') {
        $lastNativeExtent = $Matches[1]
    } elseif ($line -match 'stage=scene .*physical_px=([0-9]+x[0-9]+)') {
        if ($Matches[1] -ne $lastNativeExtent) { $resizeMismatches++ }
    }
}
$summary = [pscustomobject]@{
    Window = [int64]$window
    StartedAt = $startedAt.ToString('o')
    FinishedAt = $finishedAt.ToString('o')
    ResizeIterations = $ResizeIterations
    ClickRounds = $clickRounds
    LogBytes = ([Text.Encoding]::UTF8.GetByteCount($slice))
    BudgetHits = ([regex]::Matches($slice, 'generation budget hit')).Count
    ResizeNativeEvents = ([regex]::Matches($slice, 'stage=native')).Count
    ResizeSceneCommits = ([regex]::Matches($slice, 'stage=scene ')).Count
    SceneGenerationLag = ([regex]::Matches($slice, 'scene_generation_lag')).Count
    IntermediateScenes = ([regex]::Matches($slice, 'intermediate_scene')).Count
    Warnings = ([regex]::Matches($slice, ' WARN ')).Count
    Errors = ([regex]::Matches($slice, ' ERROR ')).Count
    SlowOwnerLogOutput = ([regex]::Matches($slice, 'log_output: [0-9.]+(µs|ms)')).Count
    LogDroppedEvents = ([regex]::Matches($slice, 'dropped [0-9]+ events')).Count
    ResizeCoverageMismatches = $resizeMismatches
    Before = $before
    After = $after
    LogSlice = $slicePath
}
$summary | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'summary.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 4
