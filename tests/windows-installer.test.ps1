$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\..\scripts\install-windows.ps1"
$temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testRoot = Join-Path $temporaryBase ('naab-installer-test-' + [guid]::NewGuid().ToString('N'))
$registryPrefix = 'Software\NAABInstallerTests\' + [guid]::NewGuid().ToString('N')
$package = Join-Path $testRoot "package with spaces 'quote'"
$localData = Join-Path $testRoot 'local-data'
$binary = Join-Path $package 'companion\naab-companion.exe'
$idA = 'a' * 32
$idB = 'b' * 32
function Check([bool]$Value, [string]$Message) { if (!$Value) { throw $Message } }
function Run([string]$Browser = 'chrome', [string]$Id = $idA, [switch]$Preview, [switch]$Uninstall) {
    Invoke-NaabRegistration -PackageRoot $package -LocalData $localData -RegistryPrefix $registryPrefix -Browser $Browser -ExtensionId $Id -Preview:$Preview -Uninstall:$Uninstall
}
function Fails([scriptblock]$Action) { $failed = $false; try { & $Action | Out-Null } catch { $failed = $true }; Check $failed 'Expected installer failure' }
try {
    New-Item -ItemType Directory -Path (Split-Path $binary) -Force | Out-Null
    [IO.File]::WriteAllText($binary, 'nonexecutable installer fixture; must never be run')
    Fails { Run -Id 'invalid' }
    Fails { Run -Id ('A' * 32) }
    Run -Preview | Out-Null
    Check (!(Test-Path -LiteralPath $localData)) 'Preview changed registration files'
    Run | Out-Null
    $manifest = Join-Path $localData 'NAAB\chrome\com.naab.companion.json'
    $original = [IO.File]::ReadAllText($manifest)
    $parsed = $original | ConvertFrom-Json
    Check ($parsed.path -ceq $binary) 'Binary path was not preserved'
    Check ($parsed.allowed_origins[0] -ceq "chrome-extension://$idA/") 'Origin mismatch'
    Run | Out-Null
    Check ([IO.File]::ReadAllText($manifest) -ceq $original) 'Repeated install changed manifest'
    Run -Browser edge | Out-Null
    $edgeManifest = Join-Path $localData 'NAAB\edge\com.naab.companion.json'
    Check (Test-Path -LiteralPath $edgeManifest) 'Separate Edge manifest missing'
    Fails { Run -Id $idB }
    Fails { Run -Id $idB -Uninstall }
    Check ([IO.File]::ReadAllText($manifest) -ceq $original) 'Conflicting identity was modified'
    $keyPath = "$registryPrefix\Google\Chrome\NativeMessagingHosts\com.naab.companion"
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($keyPath, $true)
    try { $key.SetValue('', 'C:\other\manifest.json', [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
    Fails { Run }
    Fails { Run -Uninstall }
    Check ([IO.File]::ReadAllText($manifest) -ceq $original) 'Conflicting registry changed manifest'
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($keyPath, $true)
    try { $key.SetValue('', $manifest, [Microsoft.Win32.RegistryValueKind]::String); $key.SetValue('other-value', 'preserve') } finally { $key.Dispose() }
    [IO.File]::WriteAllText($manifest, '{invalid json')
    Fails { Run }
    Fails { Run -Uninstall }
    Check ([IO.File]::ReadAllText($manifest) -ceq '{invalid json') 'Malformed manifest was modified'
    [IO.File]::WriteAllText($manifest, $original)
    # Updating in place changes executable bytes, not registration identity.
    [IO.File]::WriteAllText($binary, 'updated fixture')
    Run | Out-Null
    Check ([IO.File]::ReadAllText($manifest) -ceq $original) 'In-place update changed registration'
    Run -Uninstall -Preview | Out-Null
    Check (Test-Path -LiteralPath $manifest) 'Preview uninstall removed manifest'
    Remove-Item -LiteralPath $binary
    Fails { Run }
    Run -Uninstall | Out-Null
    Check (!(Test-Path -LiteralPath $manifest)) 'Uninstall retained matching manifest'
    Check (Test-Path -LiteralPath $edgeManifest) 'Chrome uninstall changed Edge'
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($keyPath)
    try { Check ($key.GetValue('other-value') -ceq 'preserve') 'Uninstall deleted unrelated registry value'; Check (!($key.GetValueNames() -contains '')) 'Uninstall retained default registration' } finally { $key.Dispose() }
    Run -Uninstall | Out-Null
    Run -Browser edge -Uninstall | Out-Null
    Write-Output 'PASS: Windows installer preview, install/repeat/update, conflicts, separate browsers, and identity-checked removal.'
} finally {
    # Both recursive cleanup targets are freshly generated isolated test roots.
    if (!$registryPrefix.StartsWith('Software\NAABInstallerTests\') -or $registryPrefix.Split('\')[-1] -notmatch '^[a-f0-9]{32}$') { throw 'Unsafe registry cleanup target' }
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($registryPrefix, $false)
    if ([IO.Path]::GetDirectoryName($testRoot) -ine $temporaryBase.TrimEnd('\') -or !(Split-Path $testRoot -Leaf).StartsWith('naab-installer-test-')) { throw 'Unsafe test cleanup target' }
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
