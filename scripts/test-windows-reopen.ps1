param([Parameter(Mandatory)][string]$App)

$ErrorActionPreference = "Stop"
$previousState = $env:LUDOLPH_DIR
$env:LUDOLPH_DIR = Join-Path ([IO.Path]::GetTempPath()) ("ludolph-reopen-" + [Guid]::NewGuid())
$first = $null
$second = $null
$stderr = Join-Path $env:LUDOLPH_DIR "app.stderr"

# MainWindowHandle can switch to an untitled helper after Ludolph's window closes.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class LudolphReopenWindow {
    [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string className, string title);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    private static extern bool PostMessageW(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);

    public static IntPtr Find(int processId) {
        IntPtr window = IntPtr.Zero;
        while ((window = FindWindowExW(IntPtr.Zero, window, null, "Ludolph")) != IntPtr.Zero) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == (uint)processId && IsWindowVisible(window)) return window;
        }
        return IntPtr.Zero;
    }

    public static bool Close(IntPtr window) {
        return PostMessageW(window, 0x0010, UIntPtr.Zero, IntPtr.Zero);
    }
}
'@

function Wait-For([scriptblock]$Condition, [string]$Failure) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw $Failure
}

try {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    New-Item -ItemType Directory -Path $env:LUDOLPH_DIR | Out-Null
    $first = Start-Process $App -RedirectStandardError $stderr -PassThru
    if (!$first.WaitForInputIdle(30000)) { throw "The app did not finish starting its message loop" }
    Wait-For {
        $first.Refresh()
        if ($first.HasExited) { throw "First launch exited with $($first.ExitCode)" }
        [LudolphReopenWindow]::Find($first.Id) -ne [IntPtr]::Zero
    } "The main window didn't open"
    Write-Output "First window: $($timer.ElapsedMilliseconds) ms, PID $($first.Id), handle $([LudolphReopenWindow]::Find($first.Id)), title Ludolph"

    for ($cycle = 1; $cycle -le 3; $cycle++) {
        $window = [LudolphReopenWindow]::Find($first.Id)
        if ($window -eq [IntPtr]::Zero -or ![LudolphReopenWindow]::Close($window)) {
            throw "Couldn't send the close request to Ludolph's window"
        }
        Wait-For {
            $first.Refresh()
            if ($first.HasExited) { throw "Closing the window ended the app" }
            [LudolphReopenWindow]::Find($first.Id) -eq [IntPtr]::Zero
        } "The main window didn't close"

        $timer.Restart()
        $second = Start-Process $App -PassThru
        if (!$second.WaitForExit(30000)) { throw "A second app instance stayed running" }
        $second.WaitForExit()
        if ($second.ExitCode -ne 0) { throw "The second launch exited with $($second.ExitCode)" }
        Wait-For {
            $first.Refresh()
            if ($first.HasExited) { throw "The original app exited during reopen" }
            [LudolphReopenWindow]::Find($first.Id) -ne [IntPtr]::Zero
        } "Launching again didn't restore the original app's window"
        Write-Output "Reopen cycle ${cycle}: $($timer.ElapsedMilliseconds) ms, original PID $($first.Id)"
        $second.Dispose()
        $second = $null
    }
} catch {
    if ($null -ne $first) {
        $first.Refresh()
        if ($first.HasExited) {
            Write-Output "App at failure: PID $($first.Id), exit code $($first.ExitCode)"
        } else {
            Write-Output "Window at failure: PID $($first.Id), visible Ludolph handle $([LudolphReopenWindow]::Find($first.Id)), .NET candidate handle $($first.MainWindowHandle), title $($first.MainWindowTitle), responding $($first.Responding)"
        }
    }
    if (Test-Path $stderr) { Get-Content $stderr }
    throw
} finally {
    foreach ($process in @($second, $first)) {
        if ($null -ne $process) {
            if (!$process.HasExited) {
                & taskkill.exe /PID $process.Id /T /F | Out-Null
                $process.WaitForExit()
            }
            $process.Dispose()
        }
    }
    Remove-Item -Recurse -Force $env:LUDOLPH_DIR -ErrorAction SilentlyContinue
    $env:LUDOLPH_DIR = $previousState
}
