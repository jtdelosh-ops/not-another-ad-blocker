# Opt-in independent recovery for the Windows DNS preview.
# Run in an Administrator PowerShell on a controlled Windows test machine.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Status', 'Uninstall')]
    [string]$Action,
    [string]$SourceBinary
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$taskName = 'NAAB-DNS-Preview-Recovery'
$directory = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'NAAB-DNS-Preview'
$installedBinary = Join-Path $directory 'recovery-helper.exe'
$journal = Join-Path $directory 'recovery.json'

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this setup in an Administrator PowerShell.'
    }
}

function Get-RecoveryTask {
    return Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
}

function Check-InstalledTask {
    $task = Get-RecoveryTask
    if ($null -eq $task -or -not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) { return $false }
    $actions = @($task.Actions)
    $triggers = @($task.Triggers)
    $hasStartup = @($triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskBootTrigger' }).Count -eq 1
    $hasRepeat = @($triggers | Where-Object {
        $_.CimClass.CimClassName -eq 'MSFT_TaskTimeTrigger' -and
        $_.Repetition.Interval -eq 'PT1M' -and
        [string]::IsNullOrEmpty($_.Repetition.Duration)
    }).Count -eq 1
    return ($task.State -ne 'Disabled' -and
        $task.Principal.UserId -in @('SYSTEM', 'NT AUTHORITY\SYSTEM', 'S-1-5-18') -and
        $task.Principal.RunLevel -eq 'Highest' -and
        $actions.Count -eq 1 -and
        $actions[0].Execute -ieq $installedBinary -and
        $actions[0].Arguments -ceq 'recover-if-needed' -and
        $triggers.Count -eq 2 -and $hasStartup -and $hasRepeat)
}

Require-Administrator
if ($Action -eq 'Status') {
    $task = Get-RecoveryTask
    [pscustomobject]@{
        Installed = [bool](Check-InstalledTask)
        TaskPresent = $null -ne $task
        ProtectedBinaryPresent = Test-Path -LiteralPath $installedBinary -PathType Leaf
        RecoveryPending = Test-Path -LiteralPath $journal -PathType Leaf
        TaskState = if ($null -ne $task) { [string]$task.State } else { 'Absent' }
    } | Format-List
    exit 0
}

if ($Action -eq 'Install') {
    if ($null -ne (Get-RecoveryTask)) { throw 'A recovery task already exists. Inspect its status before changing it.' }
    if ([string]::IsNullOrWhiteSpace($SourceBinary)) {
        $SourceBinary = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..\companion\target\debug\naab-dns-windows.exe'
    }
    $source = (Resolve-Path -LiteralPath $SourceBinary -ErrorAction Stop).ProviderPath
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw 'Source binary is not a file.' }
    if ((Get-Item -LiteralPath $source).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Source binary must not be a link.' }
    $prepared = @(& $source recovery-dir)
    if ($LASTEXITCODE -ne 0 -or $prepared.Count -ne 1 -or $prepared[0] -ine $directory) {
        throw 'The helper did not create or confirm the fixed protected recovery directory.'
    }
    if (Test-Path -LiteralPath $installedBinary) { throw 'A recovery binary already exists; inspect it before installing.' }
    $sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $created = $false
    try {
        [IO.File]::Copy($source, $installedBinary, $false)
        $created = $true
        $fileAcl = Get-Acl -LiteralPath $installedBinary
        $fileAcl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
        Set-Acl -LiteralPath $installedBinary -AclObject $fileAcl -ErrorAction Stop
        if ((Get-FileHash -LiteralPath $installedBinary -Algorithm SHA256).Hash -ne $sourceHash) {
            throw 'Protected copy does not match the selected binary.'
        }
        # Secure-Directory checks the new file's owner and ACL before accepting it.
        $checked = @(& $installedBinary recovery-dir)
        if ($LASTEXITCODE -ne 0 -or $checked.Count -ne 1 -or $checked[0] -ine $directory) {
            throw 'Protected recovery executable failed storage validation.'
        }
        $startup = New-ScheduledTaskTrigger -AtStartup
        # On Windows 10/11, omitted RepetitionDuration means repeat indefinitely.
        $repeat = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
            -RepetitionInterval (New-TimeSpan -Minutes 1)
        $taskAction = New-ScheduledTaskAction -Execute $installedBinary -Argument 'recover-if-needed'
        $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries `
            -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2) `
            -MultipleInstances IgnoreNew
        Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger @($startup, $repeat) `
            -Settings $settings -User 'SYSTEM' -RunLevel Highest `
            -Description 'Restore NAAB DNS preview settings after helper exit or reboot.' -ErrorAction Stop | Out-Null
        if (-not (Check-InstalledTask)) { throw 'Registered task did not match the required recovery configuration.' }
        Write-Host 'Independent Windows DNS recovery installed. Task runs at startup and every minute.'
        Write-Host "Protected helper: $installedBinary"
    } catch {
        $failure = $_
        if ($null -ne (Get-RecoveryTask)) {
            try { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction Stop }
            catch { throw "Recovery task verification failed, and task removal failed. Inspect $taskName before retrying. Original error: $failure" }
        }
        if ($created) { Remove-Item -LiteralPath $installedBinary -Force -ErrorAction Stop }
        throw $failure
    }
    exit 0
}

if (-not (Check-InstalledTask)) { throw 'Recovery task is absent or does not match; inspect it before uninstalling.' }
if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) { throw 'Protected helper is missing; retain the task and inspect recovery state.' }
& $installedBinary recover-if-needed
if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $journal)) {
    throw 'Recovery is active or incomplete. Keep the task installed and resolve the saved record first.'
}
Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction Stop
Remove-Item -LiteralPath $installedBinary -Force -ErrorAction Stop
Write-Host 'Independent Windows DNS recovery uninstalled after confirming no pending record.'
