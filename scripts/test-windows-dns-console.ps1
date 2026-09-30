# Test-only: real console signals with in-memory DNS and temporary journals.
param([Parameter(Mandatory = $true)][string]$FixtureExecutable)
$ErrorActionPreference = 'Stop'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('naab-console-test-' + [guid]::NewGuid().ToString('N'))
$scratch = [IO.Path]::GetFullPath($scratch)
New-Item -ItemType Directory -Path $scratch | Out-Null
$passed = $false
$shell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
function Quote-Ps([string]$Value) { "'" + $Value.Replace("'", "''") + "'" }
function Wait-Marker([string]$Path, $Process) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Test-Path -LiteralPath $Path)) {
        if ($Process.HasExited) { throw "Fixture exited before $Path (exit $($Process.ExitCode))" }
        if ([DateTime]::UtcNow -gt $deadline) { throw "Timed out waiting for $Path" }
        Start-Sleep -Milliseconds 50
    }
}
try {
    foreach ($case in @('single', 'repeated', 'during-enable', 'restore-failure', 'break', 'timeout', 'panic')) {
        $directory = Join-Path $scratch $case
        New-Item -ItemType Directory -Path $directory | Out-Null
        $script = Join-Path $directory 'launch.ps1'
        # Clear an inherited Ctrl+C-ignore flag so this behaves like an ordinary
        # interactive PowerShell console even when Cargo runs under a CI launcher.
        $setup = @'
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class ConsoleFixture { [DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add); }'
[ConsoleFixture]::SetConsoleCtrlHandler([IntPtr]::Zero, $false) | Out-Null
'@
        $body = $setup + "`n" + '$env:NAAB_CONSOLE_FIXTURE = ' + (Quote-Ps $directory) + "`n" +
            '$env:NAAB_CONSOLE_CASE = ' + (Quote-Ps $case) + "`n" +
            '& ' + (Quote-Ps $FixtureExecutable) + ' --exact console_fixture --nocapture'
        Set-Content -LiteralPath $script -Value $body -Encoding UTF8
        $process = Start-Process -FilePath $shell -ArgumentList @('-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $script + '"')) -WindowStyle Hidden -PassThru
        # Cache the handle while this is definitely our process, preventing PID
        # reuse from making failure cleanup target an unrelated process.
        $null = $process.Handle
        try {
            if ($case -notin @('timeout', 'panic')) {
                $marker = if ($case -eq 'during-enable') { 'applying' } else { 'active' }
                Wait-Marker (Join-Path $directory $marker) $process
                $env:NAAB_CONSOLE_TARGET_PID = [string]$process.Id
                $env:NAAB_CONSOLE_PARENT_PID = [string]$PID
                $env:NAAB_CONSOLE_EVENT = if ($case -eq 'break') { '1' } else { '0' }
                & $FixtureExecutable --exact console_signal_helper --nocapture | Out-Null
                if ($LASTEXITCODE -ne 0) { throw 'Console signal helper failed' }
                if ($case -eq 'repeated') {
                    Wait-Marker (Join-Path $directory 'restoring') $process
                    & $FixtureExecutable --exact console_signal_helper --nocapture | Out-Null
                    if ($LASTEXITCODE -ne 0) { throw 'Repeated console signal failed' }
                }
            }
            Wait-Marker (Join-Path $directory 'done.json') $process
            $result = Get-Content -Raw -LiteralPath (Join-Path $directory 'done.json') | ConvertFrom-Json
            if ($result.case -ne $case) { throw 'Unexpected fixture result' }
            if ($case -eq 'restore-failure') {
                if (-not $result.retained -or $result.restored -or $result.state -ne 'recoveryRequired') { throw 'Failed restoration lost its journal' }
            } elseif (-not $result.restored -or $result.retained -or $result.state -ne 'inactive') {
                throw 'Restoration did not finish'
            }
            if (-not $process.WaitForExit(5000)) { throw 'Fixture shell did not exit' }
            # PowerShell may report pipeline cancellation after Ctrl+C, regardless
            # of child success. The child's checked result is the cleanup evidence.
            Write-Output "$case PASS: restored=$($result.restored), retained=$($result.retained)"
        } finally {
            if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
            $process.Dispose()
        }
    }
    $passed = $true
} finally {
    if ($passed) {
        $parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
        if ([IO.Path]::GetDirectoryName($scratch) -ne $parent -or [IO.Path]::GetFileName($scratch) -notlike 'naab-console-test-*') { throw 'Unexpected scratch cleanup path' }
        Remove-Item -LiteralPath $scratch -Recurse -Force
    } else {
        Write-Warning "Console fixture evidence retained at $scratch"
    }
}
