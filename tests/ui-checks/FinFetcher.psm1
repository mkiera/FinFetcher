Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class FfWin32 {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool GetProcessMitigationPolicy(IntPtr process, int policy, out uint value, UIntPtr size);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@

$script:AE = [System.Windows.Automation.AutomationElement]
$script:Descendants = [System.Windows.Automation.TreeScope]::Descendants
$script:Proc = $null
$script:Shot = 0

function Get-FfOutDir {
    $dir = $env:FF_UI_OUT
    if (-not $dir) { $dir = Join-Path ([IO.Path]::GetTempPath()) 'finfetcher-ui-out' }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return $dir
}

function Get-FfDataRoot {
    $root = $env:FF_UI_DATA
    if (-not $root) { $root = Join-Path ([IO.Path]::GetTempPath()) 'finfetcher-ui-data' }
    return $root
}

function Assert-FfAllowed {
    if ($env:GITHUB_ACTIONS -ne 'true' -and $env:FF_UI_ALLOW_LOCAL -ne '1') {
        throw 'The UI checks launch FinFetcher windows and change the Downloads folder setting. They run only on CI, or with FF_UI_ALLOW_LOCAL=1 in a throwaway Windows session.'
    }
}

function Initialize-FfProfile {
    # Creates an empty data folder for this check and points FINFETCHER_DATA_DIR at it, so the app
    # never reads or writes %APPDATA%\FinFetcher.
    param(
        [hashtable]$Settings = @{},
        # Written as config.json instead of the generated settings.
        [string]$RawConfig,
        # Copies ffmpeg.exe and ffprobe.exe into <data>\ffmpeg, which the app finds without ffmpeg_path.
        [switch]$BundledFfmpeg
    )
    Assert-FfAllowed

    $ffmpegDir = $env:FF_UI_FFMPEG_DIR
    if (-not $ffmpegDir -or -not (Test-Path (Join-Path $ffmpegDir 'ffmpeg.exe')) -or -not (Test-Path (Join-Path $ffmpegDir 'ffprobe.exe'))) {
        throw "FF_UI_FFMPEG_DIR does not hold ffmpeg.exe and ffprobe.exe: '$ffmpegDir'"
    }
    $ytdlp = $env:FF_UI_YTDLP
    if (-not $ytdlp -or -not (Test-Path $ytdlp)) { throw "FF_UI_YTDLP does not point at yt-dlp.exe: '$ytdlp'" }

    $root = Get-FfDataRoot
    if (Test-Path $root) { Remove-Item -Recurse -Force $root }
    New-Item -ItemType Directory -Force -Path $root | Out-Null

    # A pinned yt-dlp in <data>\bin keeps the app from calling GitHub to install one.
    $bin = Join-Path $root 'bin'
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    Copy-Item $ytdlp (Join-Path $bin 'yt-dlp.exe')

    if ($BundledFfmpeg) {
        $local = Join-Path $root 'ffmpeg'
        New-Item -ItemType Directory -Force -Path $local | Out-Null
        foreach ($n in 'ffmpeg.exe', 'ffprobe.exe') { Copy-Item (Join-Path $ffmpegDir $n) (Join-Path $local $n) }
    }

    $config = Join-Path $root 'config.json'
    if ($PSBoundParameters.ContainsKey('RawConfig')) {
        [IO.File]::WriteAllText($config, $RawConfig)
    } else {
        # Update checks and yt-dlp updates stay off unless a check turns them on: they call the
        # GitHub API without a token, which allows 60 requests an hour per runner address.
        $s = [ordered]@{
            ffmpeg_path        = $ffmpegDir
            update_channel     = 'stable'
            auto_check_updates = $false
            auto_update_ytdlp  = $false
            log_to_file        = $false
        }
        foreach ($k in $Settings.Keys) { $s[$k] = $Settings[$k] }
        $s | ConvertTo-Json | Set-Content -Encoding utf8 $config
    }

    $env:FINFETCHER_DATA_DIR = $root
    [pscustomobject]@{ Root = $root; Config = $config }
}

function Start-Ff {
    param([int]$TimeoutSeconds = 90, [int]$Width = 760, [int]$Height = 1000)
    Assert-FfAllowed

    $exe = $env:FF_UI_APP
    if (-not $exe -or -not (Test-Path $exe)) { throw "FF_UI_APP does not point at the app: '$exe'" }
    if (-not $env:FINFETCHER_DATA_DIR) { throw 'Call Initialize-FfProfile before Start-Ff' }

    $out = Get-FfOutDir
    $script:Proc = Start-Process -FilePath $exe -PassThru `
        -RedirectStandardOutput (Join-Path $out 'app-stdout.txt') `
        -RedirectStandardError (Join-Path $out 'app-stderr.txt')
    return Connect-Ff -Process $script:Proc -TimeoutSeconds $TimeoutSeconds -Width $Width -Height $Height
}

function Connect-Ff {
    # Takes over a FinFetcher that something else started, such as Setup after an update.
    param(
        [Parameter(Mandatory)][System.Diagnostics.Process]$Process,
        [int]$TimeoutSeconds = 90, [int]$Width = 760, [int]$Height = 1000
    )
    $script:Proc = $Process
    $window = Get-FfWindow -TimeoutSeconds $TimeoutSeconds

    # The configured 700x1200 window is taller than the runner's screen, which puts the lower
    # half of the page off screen where mouse clicks cannot reach it.
    $h = [IntPtr]$window.Current.NativeWindowHandle
    [FfWin32]::SetWindowPos($h, [IntPtr]::Zero, 20, 20, $Width, $Height, 0x0004) | Out-Null
    [FfWin32]::SetForegroundWindow($h) | Out-Null

    # The window appears before WebView2 loads the page and before checkSetup answers.
    Wait-Ff -TimeoutSeconds $TimeoutSeconds -Message 'the page shows the main view or the setup screen' {
        (Find-FfElement -Id 'urlInput' -TimeoutSeconds 0 -Optional) -or (Find-FfElement -Id 'installBtn' -TimeoutSeconds 0 -Optional)
    } | Out-Null
    Start-Sleep -Seconds 2
    return $script:Proc
}

function Stop-Ff {
    # Closes the window the way a user does. Returns $true when the app exited on its own.
    param([int]$TimeoutSeconds = 20)
    $procs = @(Get-Process FinFetcher -ErrorAction SilentlyContinue)
    $normal = $true
    foreach ($p in $procs) {
        $h = $p.MainWindowHandle
        if ($h -ne [IntPtr]::Zero) { [FfWin32]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
        else { $p.CloseMainWindow() | Out-Null }
    }
    foreach ($p in $procs) {
        if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
            Write-Host "FinFetcher $($p.Id) did not exit within $TimeoutSeconds s, killing it"
            $p.Kill()
            $normal = $false
        }
    }
    $script:Proc = $null
    return $normal
}

function Get-FfProcess {
    if ($script:Proc -and -not $script:Proc.HasExited) { return $script:Proc }
    $p = Get-Process FinFetcher -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) { throw 'FinFetcher is not running' }
    return $p
}

function Get-FfWindow {
    param([int]$TimeoutSeconds = 10)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $proc = Get-FfProcess
        $cond = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $proc.Id)
        foreach ($w in $AE::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)) {
            if ($w.Current.Name -eq 'FinFetcher') { return $w }
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    throw "no FinFetcher window for process $((Get-FfProcess).Id)"
}

function Find-FfElement {
    # WebView2 exposes an HTML id as AutomationId and the visible text as Name. Elements hidden
    # with display:none are not in the tree.
    param(
        [string]$Name,
        # Regular expression on Name, for text that carries a version or a count.
        [string]$NameLike,
        [string]$Id,
        [string]$ControlType,
        [int]$Index = 0,
        # Search inside this element instead of the whole window.
        $Scope,
        [int]$TimeoutSeconds = 15,
        [switch]$Optional
    )

    $conds = @()
    if ($Name) { $conds += New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $Name) }
    if ($Id) { $conds += New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, $Id) }
    if ($ControlType) {
        $ct = [System.Windows.Automation.ControlType]::$ControlType
        $conds += New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $ct)
    }
    if ($conds.Count -eq 0 -and -not $NameLike) { throw 'Find-FfElement needs -Name, -NameLike, -Id or -ControlType' }
    $cond = switch ($conds.Count) {
        0 { [System.Windows.Automation.Condition]::TrueCondition }
        1 { $conds[0] }
        default { New-Object System.Windows.Automation.AndCondition($conds) }
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ($true) {
        $root = if ($Scope) { $Scope } else { Get-FfWindow }
        $found = @($root.FindAll($Descendants, $cond))
        if ($NameLike) { $found = @($found | Where-Object { $_.Current.Name -match $NameLike }) }
        if ($found.Count -gt $Index) { return $found[$Index] }
        if ((Get-Date) -ge $deadline) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($Optional) { return $null }
    throw "no element name='$Name' like='$NameLike' id='$Id' type='$ControlType' index=$Index"
}

function Get-FfText {
    # The element's Name, or when that is empty the Names of the text inside it.
    param([Parameter(Mandatory, ValueFromPipeline)]$Element)
    process {
        $n = $Element.Current.Name
        if ($n) { return $n }
        $obj = $null
        if ($Element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$obj) -and $obj.Current.Value) {
            return $obj.Current.Value
        }
        $parts = foreach ($e in $Element.FindAll($Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
            if ($e.Current.Name) { $e.Current.Name }
        }
        return (@($parts) | Select-Object -Unique) -join "`n"
    }
}

function Get-FfPattern($Element, $Pattern) {
    $obj = $null
    if (-not $Element.TryGetCurrentPattern($Pattern, [ref]$obj)) { return $null }
    return $obj
}

function Invoke-FfMouseClick {
    param([Parameter(Mandatory, ValueFromPipeline)]$Element)
    process {
        $scroll = Get-FfPattern $Element ([System.Windows.Automation.ScrollItemPattern]::Pattern)
        if ($scroll) { try { $scroll.ScrollIntoView() } catch { } ; Start-Sleep -Milliseconds 300 }
        $w = Get-FfWindow
        [FfWin32]::SetForegroundWindow([IntPtr]$w.Current.NativeWindowHandle) | Out-Null
        $r = $Element.Current.BoundingRectangle
        if ($r.IsEmpty -or $r.Width -le 0) { throw "element '$($Element.Current.Name)' has no on-screen area to click" }
        $x = [int]($r.X + $r.Width / 2)
        $y = [int]($r.Y + $r.Height / 2)
        [FfWin32]::SetCursorPos($x, $y) | Out-Null
        Start-Sleep -Milliseconds 100
        [FfWin32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
        [FfWin32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 500
    }
}

function Invoke-FfElement {
    # Clicks through InvokePattern, or with the mouse when the element has none.
    param([Parameter(Mandatory, ValueFromPipeline)]$Element)
    process {
        $p = Get-FfPattern $Element ([System.Windows.Automation.InvokePattern]::Pattern)
        if ($p) { $p.Invoke(); Start-Sleep -Milliseconds 500 }
        else { $Element | Invoke-FfMouseClick }
    }
}

function Switch-FfElement {
    param([Parameter(Mandatory, ValueFromPipeline)]$Element)
    process {
        $p = Get-FfPattern $Element ([System.Windows.Automation.TogglePattern]::Pattern)
        if ($p) { $p.Toggle(); Start-Sleep -Milliseconds 500 }
        else { $Element | Invoke-FfElement }
    }
}

function Set-FfText {
    # Sets the value, then types it when WebView2 ignored SetValue.
    param([Parameter(Mandatory, ValueFromPipeline)]$Element, [Parameter(Mandatory)][string]$Value)
    process {
        $p = Get-FfPattern $Element ([System.Windows.Automation.ValuePattern]::Pattern)
        if ($p) { $p.SetValue($Value); Start-Sleep -Milliseconds 300 }
        if (-not $p -or $p.Current.Value -ne $Value) {
            $Element.SetFocus()
            Send-FfKeys '^a'
            Send-FfKeys ($Value -replace '([+^%~(){}\[\]])', '{$1}')
        }
        if ($p -and $p.Current.Value -ne $Value) { throw "text box holds '$($p.Current.Value)' instead of '$Value'" }
    }
}

function Send-FfKeys {
    # SendKeys syntax: ^ is Ctrl, + is Shift, % is Alt.
    # -Focus puts keyboard focus inside the WebView2 page first, so page key handlers see the keys.
    param([Parameter(Mandatory)][string]$Keys, $Focus)
    $w = Get-FfWindow
    [FfWin32]::SetForegroundWindow([IntPtr]$w.Current.NativeWindowHandle) | Out-Null
    if ($Focus) { $Focus.SetFocus() }
    Start-Sleep -Milliseconds 200
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Start-Sleep -Milliseconds 400
}

function Wait-Ff {
    # Runs the block until it returns something truthy, and returns that.
    param(
        [Parameter(Mandatory, Position = 0)][scriptblock]$Until,
        [int]$TimeoutSeconds = 30,
        [Parameter(Mandatory)][string]$Message,
        [int]$IntervalMilliseconds = 500
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ($true) {
        $result = & $Until
        if ($result) { return $result }
        if ((Get-Date) -ge $deadline) { throw "timed out after $TimeoutSeconds s waiting for $Message" }
        Start-Sleep -Milliseconds $IntervalMilliseconds
    }
}

function Get-FfTree {
    # Writes every named control to <out>\<name>.txt and returns the lines.
    param([string]$Name = 'tree')
    $scope = Get-FfWindow
    $lines = foreach ($e in $scope.FindAll($Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
        $c = $e.Current
        if ($c.Name -or $c.AutomationId) {
            '{0,-12} enabled={1,-5} offscreen={2,-5} id={3,-26} {4}' -f ($c.ControlType.ProgrammaticName -replace 'ControlType\.', ''), $c.IsEnabled, $c.IsOffscreen, $c.AutomationId, ($c.Name -replace '\s+', ' ')
        }
    }
    $lines | Set-Content -Encoding utf8 (Join-Path (Get-FfOutDir) "$Name.txt")
    return $lines
}

function Save-FfScreenshot {
    # Captures the app window with PrintWindow, so it works when the window is covered.
    param([Parameter(Mandatory)][string]$Name)
    $el = Get-FfWindow
    $h = [IntPtr]$el.Current.NativeWindowHandle
    [FfWin32]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 600

    $r = New-Object FfWin32+RECT
    [FfWin32]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.R - $r.L
    $ht = $r.B - $r.T
    $bmp = New-Object System.Drawing.Bitmap($w, $ht)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $dc = $g.GetHdc()
    # 2 = PW_RENDERFULLCONTENT, without which the WebView2 surface comes out black.
    [FfWin32]::PrintWindow($h, $dc, 2) | Out-Null
    $g.ReleaseHdc($dc)
    $g.Dispose()
    $script:Shot++
    $path = Join-Path (Get-FfOutDir) ('{0:D2}-{1}.png' -f $script:Shot, $Name)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "screenshot $path ($w x $ht)"
    return $path
}

function Save-FfDesktopScreenshot {
    # The whole screen, for message boxes and anything outside the app window.
    param([Parameter(Mandatory)][string]$Name)
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $g.Dispose()
    $script:Shot++
    $path = Join-Path (Get-FfOutDir) ('{0:D2}-{1}.png' -f $script:Shot, $Name)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "screenshot $path ($($b.Width) x $($b.Height))"
    return $path
}

function Open-FfSettings {
    # The gear button's text is the gear emoji, and its title attribute says Settings.
    $gear = Find-FfElement -ControlType Button -NameLike '^(⚙️?|Settings)$'
    $gear | Invoke-FfElement
    # The section divs expose no AutomationId, so wait for a button inside the Updates section.
    Find-FfElement -Id 'checkUpdatesBtn' -TimeoutSeconds 10 | Out-Null
}

function Get-FfParent {
    param([Parameter(Mandatory)]$Element)
    return [System.Windows.Automation.TreeWalker]::RawViewWalker.GetParent($Element)
}

function Save-FfTreeOnFailure {
    try { if (Get-Process FinFetcher -ErrorAction SilentlyContinue) { Get-FfTree -Name 'tree-on-failure' | Out-Null } }
    catch { Write-Host "no control list: $_" }
}

function Get-FfKnownDownloads {
    # The Downloads folder as SHGetKnownFolderPath reports it to a new process.
    if (-not ('FfShell' -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class FfShell {
    [DllImport("shell32.dll")] public static extern int SHGetKnownFolderPath([MarshalAs(UnmanagedType.LPStruct)] Guid id, uint flags, IntPtr token, out IntPtr path);
    public static string Downloads() {
        IntPtr p;
        int hr = SHGetKnownFolderPath(new Guid("374DE290-123F-4565-9164-39C4925E467B"), 0, IntPtr.Zero, out p);
        if (hr < 0) return null;
        string s = Marshal.PtrToStringUni(p);
        Marshal.FreeCoTaskMem(p);
        return s;
    }
}
"@
    }
    return [FfShell]::Downloads()
}

function Get-FfStderr {
    $path = Join-Path (Get-FfOutDir) 'app-stderr.txt'
    if (-not (Test-Path $path)) { return '' }
    # The running app still has the file open for writing.
    $stream = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try { return (New-Object IO.StreamReader($stream)).ReadToEnd() } finally { $stream.Dispose() }
    return ''
}

function Get-FfRedirectionGuard {
    # $true when Windows RedirectionGuard (ProcessRedirectionTrustPolicy) is enforced on the process.
    param([Parameter(Mandatory)][System.Diagnostics.Process]$Process)
    $value = [uint32]0
    if (-not [FfWin32]::GetProcessMitigationPolicy($Process.Handle, 16, [ref]$value, [UIntPtr]4)) {
        throw "GetProcessMitigationPolicy failed for process $($Process.Id): $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    return [bool]($value -band 1)
}

function Assert-Ff {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if (-not $Condition) { throw "check failed: $Message" }
    Write-Host "ok: $Message"
}

Export-ModuleMember -Function *-Ff*
