# Isolated tests of the actual embedded adapter functions/dispatch. All operating
# system discovery and mutation cmdlets below are in-memory doubles. No DNS writes.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$source = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot '..\companion\src\dns\system\windows.ps1')
$start = $source.IndexOf('function Require-Administrator')
$end = $source.IndexOf('# Dispatch:')
if ($start -lt 0 -or $end -lt 0) { throw 'Missing embedded test boundaries' }
. ([scriptblock]::Create($source.Substring($start, $end - $start)))
$dispatchText = $source.Substring($end).Replace('exit 0', 'return').Replace('exit 1', 'throw')
$script:dispatchBlock = [scriptblock]::Create($dispatchText)
$systemModules = 'fixture-only'
$systemNetsh = 'Invoke-NetshFixture'

function Assert($condition, [string]$message) { if (!$condition) { throw $message } }
function Throws([scriptblock]$operation) {
    $failed = $false
    try { & $operation | Out-Null } catch { $failed = $true }
    Assert $failed 'Expected rejection'
}
Assert ((Parse-Setting '' @('192.0.2.53') 'ipv4').mode -eq 'automatic') 'DHCP-derived values must not become static backups'
$parsed = Parse-Setting '2001:db8::2,2001:db8::1' @('2001:db8::2','2001:db8::1') 'ipv6'
Assert (($parsed.servers -join ',') -eq '2001:db8::2,2001:db8::1') 'Static server order changed'
Throws { Parse-Setting '192.0.2.53' @('192.0.2.54') 'ipv4' }
Throws { Parse-Setting '192.0.2.53' @('192.0.2.53') 'ipv6' }
Throws { Parse-Setting 'fe80::1%4' @('fe80::1%4') 'ipv6' }

$script:id = '00000000-0000-0000-0000-000000000001'
$script:network = '00000000-0000-0000-0000-000000000002'
$script:states = @{ ipv4 = @{mode='automatic'}; ipv6 = @{mode='static';servers=@('2001:db8::2','2001:db8::1')} }
$script:writes = @()
$script:foreign = $false
$script:extraProfile = $false
$script:domain = $false
$script:extraAdapters = @()
$script:extraDns = @()
$script:dnsEnumerationFails = $false
$script:netshExitCode = 0
function Import-Module { param($Name) }
function Require-Administrator {}
function Machine-Id { return $script:id }
function Registry-String { param($path,$name) return '' }
function Get-NetAdapter {
    param([switch]$Physical,[switch]$IncludeHidden)
    [pscustomobject]@{InterfaceGuid=$script:id;Name='Fixture';ifIndex=42;Status='Up'}
    if ($IncludeHidden) { $script:extraAdapters }
}
function Get-NetConnectionProfile {
    $id = if ($script:foreign) { $script:id } else { $script:network }
    $category = if ($script:domain) { 2 } else { 0 }
    [pscustomobject]@{InterfaceIndex=42;InstanceID=$id;NetworkCategory=$category}
    if ($script:extraProfile) { [pscustomobject]@{InterfaceIndex=43;InstanceID=$script:id;NetworkCategory=0} }
}
function Get-NetAdapterBinding { param($Name,$ComponentID) return [pscustomobject]@{Name='Fixture';Enabled=$true} }
function Get-DnsClientNrptPolicy { param([switch]$Effective) }
function Read-Setting { param($adapter,$family) return $script:states[$family] }
function Get-DnsClientServerAddress {
    param($InterfaceIndex,$AddressFamily)
    if (!$PSBoundParameters.ContainsKey('InterfaceIndex')) {
        if ($script:dnsEnumerationFails) { throw 'Fixture DNS enumeration failed' }
        foreach ($family in @('ipv4','ipv6')) {
            Get-DnsClientServerAddress -InterfaceIndex 42 -AddressFamily $family
        }
        $script:extraDns
        return
    }
    if ($InterfaceIndex -ne 42) { throw 'No MSFT_DNSClientServerAddress objects found for this interface' }
    $setting = $script:states[$AddressFamily]
    $servers = if ($setting.mode -eq 'static') { $setting.servers } elseif ($AddressFamily -eq 'ipv4') { @('192.0.2.53') } else { @('2001:db8::53') }
    [pscustomobject]@{InterfaceIndex=$InterfaceIndex;AddressFamily=$AddressFamily;ServerAddresses=$servers}
}
function Set-DnsClientServerAddress {
    param([Parameter(ValueFromPipeline=$true)]$InputObject,[string[]]$ServerAddresses,[switch]$ResetServerAddresses,[switch]$Confirm)
    process {
        Assert ($InputObject.InterfaceIndex -eq 42) 'Wrong adapter index'
        $family = $InputObject.AddressFamily
        if ($ResetServerAddresses) {
            # Observed on the controlled Windows 10 guest: even an IPv6 CIM
            # input resets both families. A regression to this API must fail.
            $script:states.ipv4 = @{mode='automatic'}
            $script:states.ipv6 = @{mode='automatic'}
        } else {
            $script:states[$family] = @{mode='static';servers=@($ServerAddresses)}
        }
        $script:writes += $family
    }
}
function Invoke-NetshFixture {
    Assert ($args.Count -eq 6) 'Unexpected netsh argument count'
    Assert ($args[0] -ceq 'interface' -and $args[1] -cin @('ipv4','ipv6') -and
        $args[2] -ceq 'set' -and $args[3] -ceq 'dnsservers' -and
        $args[4] -ceq 'name=42' -and $args[5] -ceq 'source=dhcp') 'Unexpected netsh DNS reset arguments'
    $global:LASTEXITCODE = $script:netshExitCode
    if ($script:netshExitCode -ne 0) {
        Write-Output 'Fixture netsh reset failed'
        return
    }
    $family = $args[1]
    $script:states[$family] = @{mode='automatic'}
    $script:writes += $family
    Write-Output 'Ok.'
}
function Invoke-Fixture($request) {
    $json = & $script:dispatchBlock
    return $json | ConvertFrom-Json
}
$snapshot = Invoke-Fixture @{action='snapshot';interfaceId=$script:id}
Assert ($snapshot.states.Count -eq 2) 'Both enabled families must be captured'
Assert ($script:writes.Count -eq 0) 'Snapshot wrote DNS'
# Hyper-V WAN miniports can be Up without any DNS CIM records. Windows throws
# for a per-index lookup on them, while enumeration simply has no matching rows.
$script:extraAdapters = @([pscustomobject]@{Name='WAN Miniport';ifIndex=19;Status='Up'})
Assert ((Invoke-Fixture @{action='snapshot';interfaceId=$script:id}).states.Count -eq 2) 'A miniport without DNS records blocked the snapshot'
Invoke-Fixture @{action='guard';interfaceId=$script:id} | Out-Null
# An extra Up interface with actual DNS servers must still fail closed.
$script:extraDns = @([pscustomobject]@{InterfaceIndex=19;AddressFamily='ipv4';ServerAddresses=@('192.0.2.54')})
Throws { Invoke-Fixture @{action='snapshot';interfaceId=$script:id} }
Throws { Invoke-Fixture @{action='guard';interfaceId=$script:id} }
$script:extraDns = @([pscustomobject]@{InterfaceIndex=19;AddressFamily='ipv6';ServerAddresses=@('2001:db8::54')})
Throws { Invoke-Fixture @{action='snapshot';interfaceId=$script:id} }
Throws { Invoke-Fixture @{action='guard';interfaceId=$script:id} }
$script:extraAdapters[0].Status = 'Down'
Invoke-Fixture @{action='guard';interfaceId=$script:id} | Out-Null
$script:extraAdapters[0].Status = 'Up'
$script:extraDns = @([pscustomobject]@{InterfaceIndex=19;AddressFamily='ipv4';ServerAddresses=@()})
Invoke-Fixture @{action='guard';interfaceId=$script:id} | Out-Null
$script:dnsEnumerationFails = $true
Throws { Invoke-Fixture @{action='snapshot';interfaceId=$script:id} }
Throws { Invoke-Fixture @{action='guard';interfaceId=$script:id} }
$script:dnsEnumerationFails = $false
$script:extraAdapters = @()
$script:extraDns = @()
Assert ($script:writes.Count -eq 0) 'Environment checks wrote DNS'
$target = @{interfaceId=$script:id;networkId=($script:id+':'+$script:network);family='ipv4'}
Invoke-Fixture @{action='cas';target=$target;expected=@{mode='automatic'};replacement=@{mode='static';servers=@('127.0.0.1')}} | Out-Null
Assert ($script:states.ipv4.servers[0] -eq '127.0.0.1') 'IPv4 activation failed'
Assert (($script:states.ipv6.servers -join ',') -eq '2001:db8::2,2001:db8::1') 'IPv4 activation changed IPv6'
Throws { Invoke-Fixture @{action='cas';target=$target;expected=@{mode='automatic'};replacement=@{mode='static';servers=@('127.0.0.1')}} }
Assert ($script:writes.Count -eq 1) 'Conflict must not write'
Invoke-Fixture @{action='cas';target=$target;expected=@{mode='static';servers=@('127.0.0.1')};replacement=@{mode='automatic'}} | Out-Null
Assert ($script:states.ipv4.mode -eq 'automatic') 'Automatic restoration became a static list'
Assert (($script:states.ipv6.servers -join ',') -eq '2001:db8::2,2001:db8::1') 'IPv4 reset changed IPv6'
$target6 = @{interfaceId=$script:id;networkId=($script:id+':'+$script:network);family='ipv6'}
$original6 = $script:states.ipv6
Invoke-Fixture @{action='cas';target=$target6;expected=$original6;replacement=@{mode='static';servers=@('::1')}} | Out-Null
Invoke-Fixture @{action='cas';target=$target6;expected=@{mode='static';servers=@('::1')};replacement=$original6} | Out-Null
Assert (($script:states.ipv6.servers -join ',') -eq '2001:db8::2,2001:db8::1') 'IPv6 restoration lost server order'
$script:foreign = $true
Assert ($null -eq (Invoke-Fixture @{action='read';target=$target}).setting) 'Changed network must be missing'
Throws { Invoke-Fixture @{action='cas';target=$target;expected=@{mode='automatic'};replacement=@{mode='static';servers=@('127.0.0.1')}} }
Assert ($script:writes.Count -eq 4) 'Changed network was written'
$script:foreign = $false
$script:extraProfile = $true
Throws { Invoke-Fixture @{action='snapshot';interfaceId=$script:id} }
Throws { Invoke-Fixture @{action='guard';interfaceId=$script:id} }
# Widening the environment must not prevent restoration of a still-matching target.
Invoke-Fixture @{action='cas';target=$target;expected=@{mode='automatic'};replacement=@{mode='static';servers=@('192.0.2.53')}} | Out-Null
Assert ($script:writes.Count -eq 5) 'Environment guard must not disable restoration'
$script:extraProfile = $false
$script:domain = $true
Throws { Invoke-Fixture @{action='snapshot';interfaceId=$script:id} }
$script:domain = $false
$script:extraProfile = $false
$script:states.ipv4 = @{mode='static';servers=@('192.0.2.53')}
$script:states.ipv6 = @{mode='static';servers=@('::1')}
Invoke-Fixture @{action='cas';target=$target6;expected=@{mode='static';servers=@('::1')};replacement=@{mode='automatic'}} | Out-Null
Assert ($script:states.ipv6.mode -eq 'automatic') 'IPv6 reset did not restore automatic mode'
Assert (($script:states.ipv4.servers -join ',') -eq '192.0.2.53') 'IPv6 reset changed static IPv4'
$script:netshExitCode = 1
$beforeFailedReset = $script:writes.Count
Throws { Invoke-Fixture @{action='cas';target=$target;expected=@{mode='static';servers=@('192.0.2.53')};replacement=@{mode='automatic'}} }
Assert ($script:writes.Count -eq $beforeFailedReset) 'Failed netsh reset unexpectedly wrote DNS'
Assert ($script:states.ipv4.mode -eq 'static') 'Failed netsh reset was reported as automatic'
$script:netshExitCode = 0
Write-Output 'Windows DNS adapter fixtures passed (mode, ordering, family isolation, conflicts, network changes, unsupported networks, miniports without DNS records, competing DNS interfaces, discovery errors). No OS DNS writes.'
