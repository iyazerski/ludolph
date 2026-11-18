param([Parameter(Mandatory)][string]$App)

$ErrorActionPreference = "Stop"
$previousState = $env:LUDOLPH_DIR
$env:LUDOLPH_DIR = Join-Path ([IO.Path]::GetTempPath()) ("ludolph-reopen-" + [Guid]::NewGuid())
$first = $null
$second = $null
$stderr = Join-Path $env:LUDOLPH_DIR "app.stderr"

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
        $first.MainWindowHandle -ne 0
    } "The main window didn't open"
    Write-Output "First window: $($timer.ElapsedMilliseconds) ms, PID $($first.Id), handle $($first.MainWindowHandle), title $($first.MainWindowTitle)"

    for ($cycle = 1; $cycle -le 3; $cycle++) {
        if (!$first.CloseMainWindow()) { throw "Couldn't close the main window" }
        Wait-For {
            $first.Refresh()
            if ($first.HasExited) { throw "Closing the window ended the app" }
            $first.MainWindowHandle -eq 0
        } "The main window didn't close"

        $timer.Restart()
        $second = Start-Process $App -PassThru
        if (!$second.WaitForExit(30000)) { throw "A second app instance stayed running" }
        $second.WaitForExit()
        if ($second.ExitCode -ne 0) { throw "The second launch exited with $($second.ExitCode)" }
        Wait-For {
            $first.Refresh()
            if ($first.HasExited) { throw "The original app exited during reopen" }
            $first.MainWindowHandle -ne 0
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
            Write-Output "Window at failure: PID $($first.Id), handle $($first.MainWindowHandle), title $($first.MainWindowTitle), responding $($first.Responding)"
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
