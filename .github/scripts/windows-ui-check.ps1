# Runs Ember on the Windows CI runner's desktop: screenshots of the settings window, and a check
# that title and fullscreen rules see real windows. Screenshots and logs go to $Out.
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Exe = (Resolve-Path $Exe).Path
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, int data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    // Windows only lets the foreground process hand out focus; a tap of Alt counts as input
    // and lifts that restriction.
    public static void Focus(IntPtr h) {
        keybd_event(0x12, 0, 0, UIntPtr.Zero);
        keybd_event(0x12, 0, 2, UIntPtr.Zero);
        ShowWindow(h, 9);
        SetForegroundWindow(h);
    }
}
"@

function Save-Screen([string] $name) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose()
    $bmp.Dispose()
}

function Wait-Pumping([int] $ms) {
    $until = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $until) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 50
    }
}

function Write-Config([string] $name, [string] $text) {
    $path = Join-Path $env:RUNNER_TEMP "$name.toml"
    [System.IO.File]::WriteAllText($path, $text) # UTF-8 without BOM
    $path
}

# Open the settings window with `config` and screenshot it from top to bottom.
function Show-Settings([string] $name, [string] $config) {
    $cfg = Write-Config $name $config
    $p = Start-Process $Exe -ArgumentList '--settings', '-c', "`"$cfg`"" -PassThru
    for ($i = 0; $i -lt 30 -and $p.MainWindowHandle -eq 0; $i++) {
        Start-Sleep 1
        $p.Refresh()
    }
    if ($p.MainWindowHandle -eq 0) { throw "the settings window didn't open" }
    Start-Sleep 3
    [Win]::Focus($p.MainWindowHandle)
    $r = New-Object Win+RECT
    [Win]::GetWindowRect($p.MainWindowHandle, [ref] $r) | Out-Null
    [Win]::SetCursorPos([int](($r.Left + $r.Right) / 2), [int](($r.Top + $r.Bottom) / 2)) | Out-Null
    for ($k = 0; $k -lt 6; $k++) {
        Start-Sleep -Milliseconds 800
        Save-Screen "settings-$name-$k"
        [Win]::mouse_event(0x0800, 0, 0, -480, [UIntPtr]::Zero) # wheel down
    }
    Stop-Process $p
    Start-Sleep 1
}

$rules = @"
[[rules]]
name = "Fullscreen apps"
fullscreen = true

[[rules]]
name = "Steam games"
steam_games = true

[[rules]]
name = "YouTube in Firefox"
processes = ["firefox"]
titles = ["YouTube"]
temperature = 5000
"@

Show-Settings 'fixed' @"
[schedule]
mode = "fixed"
bedtime = "23:00"

[schedule.days]
fri = { night_start = "23:30", night_end = "09:00" }
sat = { night_start = "23:30", night_end = "09:00" }

$rules
"@

Show-Settings 'solar' @"
[schedule]
mode = "solar"
latitude = 55.68
longitude = 12.57
sunset_offset_minutes = 30
"@

# Rules against real windows: a window with a matching title, then one covering the screen.
$cfg = Write-Config 'rules' @"
[general]
poll_interval_secs = 1

[[rules]]
name = "title-check"
titles = ["Ember CI title"]
temperature = 5000

[[rules]]
name = "fullscreen-check"
fullscreen = true
"@
$log = Join-Path $env:LOCALAPPDATA 'ember\data\ember.log'
$ember = Start-Process $Exe -ArgumentList '-c', "`"$cfg`"" -PassThru
Start-Sleep 5

$titled = New-Object System.Windows.Forms.Form
$titled.Text = 'Ember CI title'
$titled.Size = New-Object System.Drawing.Size 640, 400
$titled.Show()
[Win]::Focus($titled.Handle)
Wait-Pumping 4000
Save-Screen 'rule-title'

$full = New-Object System.Windows.Forms.Form
$full.Text = 'Fullscreen probe'
$full.FormBorderStyle = 'None'
$full.StartPosition = 'Manual'
$full.Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$full.BackColor = [System.Drawing.Color]::DarkSlateBlue
$full.Show()
[Win]::Focus($full.Handle)
Wait-Pumping 4000
Save-Screen 'rule-fullscreen'

$full.Close()
$titled.Close()
Wait-Pumping 3000
Save-Screen 'desktop-with-tray'
Stop-Process $ember
Start-Sleep 1

Copy-Item $log (Join-Path $Out 'ember.log')
$settingsLog = Join-Path $env:LOCALAPPDATA 'ember\data\settings.log'
if (Test-Path $settingsLog) { Copy-Item $settingsLog (Join-Path $Out 'settings.log') }

$text = Get-Content $log -Raw
$failed = @()
foreach ($rule in 'title-check', 'fullscreen-check') {
    if ($text -match "\(rule `"$rule`"\)") { Write-Host "ok: rule $rule matched" }
    else { $failed += $rule }
}
if ($failed) { throw "rules never matched: $($failed -join ', '). See ember.log." }
