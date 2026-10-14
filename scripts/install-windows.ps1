param(
    [string]$ExtensionId = '',
    [ValidateSet('', 'chrome', 'edge')][string]$Browser = '',
    [switch]$Uninstall,
    [switch]$Preview
)

$ErrorActionPreference = 'Stop'

function Assert-NaabPlainPath([string]$Path) {
    $candidate = [IO.Path]::GetFullPath($Path)
    while ($candidate) {
        if (Test-Path -LiteralPath $candidate) {
            if ((Get-Item -LiteralPath $candidate -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "A package or registration path is redirected: $candidate. Use a regular local folder."
            }
        }
        $candidate = [IO.Path]::GetDirectoryName($candidate)
    }
}

function Invoke-NaabRegistration {
    param(
        [Parameter(Mandatory)][string]$PackageRoot,
        [Parameter(Mandatory)][string]$ExtensionId,
        [ValidateSet('chrome', 'edge')][string]$Browser = 'chrome',
        [switch]$Uninstall,
        [switch]$Preview,
        [string]$LocalData = $env:LOCALAPPDATA,
        [Microsoft.Win32.RegistryKey]$Registry = [Microsoft.Win32.Registry]::CurrentUser,
        # Internal test seam; the command-line entry point always uses Software.
        [string]$RegistryPrefix = 'Software'
    )
    if ($ExtensionId -cnotmatch '^[a-p]{32}$') { throw 'Use the 32 lowercase letters (a-p) shown on the NAAB extension card.' }
    if (!$LocalData -or ![IO.Path]::IsPathRooted($LocalData)) { throw 'Your local application-data folder is unavailable.' }
    $PackageRoot = [IO.Path]::GetFullPath($PackageRoot)
    $binary = Join-Path $PackageRoot 'companion\naab-companion.exe'
    $directory = Join-Path $LocalData "NAAB\$Browser"
    $manifestPath = Join-Path $directory 'com.naab.companion.json'
    Assert-NaabPlainPath $binary
    Assert-NaabPlainPath $manifestPath
    if (!(Test-Path -LiteralPath (Split-Path $binary) -PathType Container)) { throw 'Keep the extracted package together, including its companion folder.' }
    if (!$Uninstall -and !(Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'The companion executable is missing. Extract the complete package.' }
    $vendor = if ($Browser -eq 'chrome') { 'Google\Chrome' } else { 'Microsoft\Edge' }
    $keyPath = "$RegistryPrefix\$vendor\NativeMessagingHosts\com.naab.companion"
    $manifest = [ordered]@{
        name = 'com.naab.companion'
        description = 'Not Another Ad Blocker local companion'
        path = $binary
        type = 'stdio'
        allowed_origins = @("chrome-extension://$ExtensionId/")
    }
    $exists = Test-Path -LiteralPath $manifestPath
    if ($exists) {
        if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw 'The registration target is not a regular file.' }
        try { $current = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json }
        catch { throw 'The existing registration is unreadable. It was left unchanged.' }
        $names = @($current.PSObject.Properties.Name | Sort-Object)
        $wanted = @($manifest.Keys | Sort-Object)
        if (($names -join ',') -cne ($wanted -join ',') -or
            $current.name -cne $manifest.name -or $current.description -cne $manifest.description -or
            $current.path -ine $binary -or $current.type -cne 'stdio' -or
            $current.allowed_origins -isnot [array] -or $current.allowed_origins.Count -ne 1 -or
            $current.allowed_origins[0] -cne $manifest.allowed_origins[0]) {
            throw 'A different NAAB registration exists. Uninstall it using its original package and extension ID first. It was left unchanged.'
        }
    }
    $key = $Registry.OpenSubKey($keyPath, $false)
    $registered = $null
    try {
        if ($null -ne $key -and $key.GetValueNames() -contains '') {
            if ($key.GetValueKind('') -ne [Microsoft.Win32.RegistryValueKind]::String) { throw 'The existing registry registration has an unexpected type.' }
            $registered = $key.GetValue('', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if (!$registered -or ![IO.Path]::IsPathRooted($registered) -or [IO.Path]::GetFullPath($registered) -ine $manifestPath) {
                throw 'This browser points to a different native-host manifest. It was left unchanged.'
            }
            if ($Uninstall -and !$exists) { throw 'The registered manifest is missing; its extension identity cannot be verified. Registration was left unchanged.' }
        }
    } finally { if ($null -ne $key) { $key.Dispose() } }
    if ($Preview) {
        Write-Output "Preview only: $Browser registration at $manifestPath"
        if (!$Uninstall) { $manifest | ConvertTo-Json -Depth 4 }
        return
    }
    if ($Uninstall) {
        if ($null -ne $registered) {
            $key = $Registry.OpenSubKey($keyPath, $true)
            try { $key.DeleteValue('', $false) } finally { $key.Dispose() }
        }
        if ($exists) { Remove-Item -LiteralPath $manifestPath }
        Write-Output "Registration removed for $Browser. Browser settings and downloaded lists were kept."
        return
    }
    if ($exists -and $null -ne $registered) { Write-Output "Already registered for $Browser. No changes needed."; return }
    $createdManifest = $false
    $temporary = $null
    try {
        if (!$exists) {
            New-Item -ItemType Directory -Path $directory -Force | Out-Null
            Assert-NaabPlainPath $manifestPath
            $temporary = Join-Path $directory ('.naab-manifest-' + [guid]::NewGuid().ToString('N') + '.tmp')
            [IO.File]::WriteAllText($temporary, ($manifest | ConvertTo-Json -Depth 4) + "`n", (New-Object Text.UTF8Encoding($false)))
            # Move without replacement; never overwrite a concurrent installer.
            [IO.File]::Move($temporary, $manifestPath)
            $createdManifest = $true
        }
        $key = $Registry.CreateSubKey($keyPath)
        try { $key.SetValue('', $manifestPath, [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
    } catch {
        if ($createdManifest) { Remove-Item -LiteralPath $manifestPath }
        throw
    } finally {
        if ($temporary -and (Test-Path -LiteralPath $temporary)) { Remove-Item -LiteralPath $temporary }
    }
    Write-Output "Registered NAAB for $Browser. Keep this package in place and click Check companion in NAAB."
}

# Dot-sourcing loads the functions for isolated integration tests only.
if ($MyInvocation.InvocationName -ne '.') {
    try {
        if (!$Browser) { $Browser = (Read-Host 'Browser: chrome or edge (Enter for chrome)').Trim(); if (!$Browser) { $Browser = 'chrome' } }
        if (!$ExtensionId) { $ExtensionId = (Read-Host 'Paste the 32-character NAAB extension ID').Trim() }
        Invoke-NaabRegistration -PackageRoot $PSScriptRoot -ExtensionId $ExtensionId -Browser $Browser -Uninstall:$Uninstall -Preview:$Preview
    } catch { Write-Error -Message $_.Exception.Message -ErrorAction Continue; exit 1 }
}
