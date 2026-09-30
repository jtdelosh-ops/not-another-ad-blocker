# Embedded by the Rust helper. Input is bounded JSON on stdin, never executable text.
# Windows PowerShell 5.1 is selected by absolute system path; profiles are disabled.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$systemModules = Join-Path ([Environment]::GetFolderPath('Windows')) 'System32\WindowsPowerShell\v1.0\Modules'
$systemNetsh = Join-Path ([Environment]::GetFolderPath('Windows')) 'System32\netsh.exe'
$env:PSModulePath = $systemModules

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -and
        $identity.User.Value -ne 'S-1-5-18') {
        throw 'Open an Administrator PowerShell or use the installed SYSTEM recovery task for the DNS settings helper.'
    }
}

function Registry-String([string]$path, [string]$name) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path)
    if ($null -eq $key) { return '' }
    try { return [string]$key.GetValue($name, '') } finally { $key.Dispose() }
}

function Machine-Id {
    return ([guid](Registry-String 'SOFTWARE\Microsoft\Cryptography' 'MachineGuid')).ToString('D')
}

function Read-Setting($adapter, [string]$family) {
    $stack = if ($family -eq 'ipv4') { 'Tcpip' } else { 'Tcpip6' }
    $guid = ([guid]$adapter.InterfaceGuid).ToString('B')
    $path = "SYSTEM\CurrentControlSet\Services\$stack\Parameters\Interfaces\$guid"
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path)
    if ($null -eq $key) { throw 'Missing DNS registry state; cannot determine automatic versus static mode.' }
    try {
        if ([string]$key.GetValue('ProfileNameServer', '') -ne '') { throw 'Per-network DNS overrides are unsupported in this preview.' }
        foreach ($childName in $key.GetSubKeyNames()) {
            $child = $key.OpenSubKey($childName)
            try {
                if ([string]$child.GetValue('NameServer', '') -ne '' -or [string]$child.GetValue('ProfileNameServer', '') -ne '') {
                    throw 'Per-network DNS overrides are unsupported in this preview.'
                }
            } finally { $child.Dispose() }
        }
        $raw = [string]$key.GetValue('NameServer', '')
    } finally { $key.Dispose() }
    $effective = @((Get-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -AddressFamily $family).ServerAddresses | ForEach-Object { ([Net.IPAddress]::Parse($_)).ToString().ToLowerInvariant() })
    return Parse-Setting $raw $effective $family
}

function Parse-Setting([string]$raw, [string[]]$effective, [string]$family) {
    if ([string]::IsNullOrWhiteSpace($raw)) { return @{ mode = 'automatic' } }
    $servers = @($raw -split '[,;\s]+' | Where-Object { $_ -ne '' } | ForEach-Object {
        if ($_.Contains('%')) { throw 'Scoped DNS addresses are unsupported in this preview.' }
        $ip = [Net.IPAddress]::Parse($_)
        if (($ip.AddressFamily -eq [Net.Sockets.AddressFamily]::InterNetwork) -ne ($family -eq 'ipv4')) {
            throw 'DNS address family does not match the registry setting.'
        }
        $ip.ToString().ToLowerInvariant()
    })
    # A static override must match what Windows actually reports; do not guess
    # around policy, encrypted-DNS/profile overrides or an in-flight OS change.
    if (($effective -join ',') -cne ($servers -join ',')) { throw 'Configured and effective DNS differ; refusing an ambiguous setting.' }
    return @{ mode = 'static'; servers = $servers }
}

function Same-Setting($left, $right) {
    if ($left.mode -cne $right.mode) { return $false }
    if ($left.mode -eq 'automatic') { return $true }
    return (($left.servers -join ',') -ceq ($right.servers -join ','))
}

function Reset-DnsFamily($adapter, [string]$family) {
    # Set-DnsClientServerAddress -ResetServerAddresses resets both families on
    # Windows 10, even when its input is the IPv6-only CIM object. Use the
    # family-specific Windows command so mixed static/automatic DNS survives.
    $output = & $systemNetsh interface $family set dnsservers "name=$($adapter.ifIndex)" 'source=dhcp' 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Failed to reset $family DNS: $($output -join ' ')" }
}

function Resolve-Target($target) {
    $parts = $target.networkId -split ':'
    if ($parts.Count -ne 2 -or $parts[0] -cne (Machine-Id)) { return $null }
    $id = ([guid]$target.interfaceId).ToString('D')
    $adapters = @(Get-NetAdapter -IncludeHidden | Where-Object { ([guid]$_.InterfaceGuid).ToString('D') -eq $id })
    if ($adapters.Count -ne 1) { return $null }
    $adapter = $adapters[0]
    $profiles = @(Get-NetConnectionProfile | Where-Object { $_.InterfaceIndex -eq $adapter.ifIndex })
    if ($profiles.Count -ne 1 -or ([guid]$profiles[0].InstanceID).ToString('D') -cne $parts[1]) { return $null }
    return $adapter
}

function Secure-Directory {
    Require-Administrator
    $parent = [Environment]::GetFolderPath('CommonApplicationData')
    $directory = Join-Path $parent 'NAAB-DNS-Preview'
    foreach ($path in @($parent, $directory)) {
        if ([IO.File]::Exists($path)) { throw 'Recovery storage path is a file.' }
        if ([IO.Directory]::Exists($path) -and (([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            throw 'Recovery storage must not use reparse points.'
        }
    }
    if (![IO.Directory]::Exists($directory)) {
        $security = New-Object Security.AccessControl.DirectorySecurity
        $security.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
        [void][IO.Directory]::CreateDirectory($directory, $security)
    }
    $acl = [IO.Directory]::GetAccessControl($directory)
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    if (!$acl.AreAccessRulesProtected -or $owner -notin @('S-1-5-18','S-1-5-32-544') -or $rules.Count -ne 2) { throw 'Recovery directory ownership or permissions are unsafe.' }
    foreach ($rule in $rules) {
        if ($rule.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544') -or $rule.AccessControlType -ne 'Allow' -or $rule.FileSystemRights -ne 'FullControl' -or $rule.InheritanceFlags -ne 'ContainerInherit, ObjectInherit' -or $rule.PropagationFlags -ne 'None') {
            throw 'Recovery directory permissions are unsafe.'
        }
    }
    # Reject preexisting child reparse points before Rust opens files.
    foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($directory)) {
        if (([IO.File]::GetAttributes($entry) -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or [IO.Directory]::Exists($entry)) { throw 'Unexpected recovery storage entry.' }
        $entryAcl = [IO.File]::GetAccessControl($entry)
        if ($entryAcl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin @('S-1-5-18','S-1-5-32-544')) { throw 'Recovery file ownership is unsafe.' }
        foreach ($rule in $entryAcl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
            if ($rule.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544')) { throw 'Recovery file permissions are unsafe.' }
        }
    }
    return $directory
}

# Dispatch: tests exercise this same code with isolated in-memory cmdlet doubles.
try {
    if ($request.action -eq 'resolverCheck') {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = New-Object Security.Principal.WindowsPrincipal($identity)
        if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or $identity.User.Value -eq 'S-1-5-18') { throw 'Run the resolver in a normal, non-administrator PowerShell. Only the settings helper needs elevation.' }
        @{ ok = $true } | ConvertTo-Json -Compress
        exit 0
    }
    if ($request.action -eq 'prepare') {
        @{ directory = (Secure-Directory) } | ConvertTo-Json -Compress
        exit 0
    }
    Import-Module (Join-Path $systemModules 'NetAdapter\NetAdapter.psd1')
    Import-Module (Join-Path $systemModules 'NetConnection\NetConnection.psd1')
    Import-Module (Join-Path $systemModules 'DnsClient\DnsClient.psd1')
    if ($request.action -in @('inspect','snapshot','guard')) {
        $profiles = @(Get-NetConnectionProfile)
        $items = @(foreach ($adapter in @(Get-NetAdapter -Physical | Where-Object { $_.Status -eq 'Up' })) {
            $matches = @($profiles | Where-Object { $_.InterfaceIndex -eq $adapter.ifIndex })
            $eligible = $matches.Count -eq 1 -and $profiles.Count -eq 1 -and $matches[0].NetworkCategory -ne 2
            @{ id = ([guid]$adapter.InterfaceGuid).ToString('D'); name = $adapter.Name; eligible = $eligible; profilesOnAdapter = $matches.Count; profilesOnComputer = $profiles.Count }
        })
        if ($request.action -eq 'inspect') {
            @{ adapters = $items; note = 'Preview requires one physical adapter with one non-domain network profile; eligibility is provisional until snapshot checks.' } | ConvertTo-Json -Depth 8 -Compress
            exit 0
        }
        $selected = @($items | Where-Object { $_.id -ceq $request.interfaceId -and $_.eligible })
        if ($selected.Count -ne 1) { throw 'Select one connected physical adapter on a single non-domain network; VPN/multiple networks are unsupported.' }
        if (@(Get-DnsClientNrptPolicy -Effective).Count -ne 0) { throw 'NRPT/domain DNS policies are unsupported in this preview.' }
        if ((Registry-String 'SYSTEM\CurrentControlSet\Services\Tcpip\Parameters' 'NameServer') -ne '' -or (Registry-String 'SOFTWARE\Policies\Microsoft\Windows NT\DNSClient' 'NameServer') -ne '') { throw 'Global or managed DNS overrides are unsupported.' }
        $adapter = @(Get-NetAdapter -Physical | Where-Object { ([guid]$_.InterfaceGuid).ToString('D') -ceq $request.interfaceId })[0]
        # Up WAN miniports may have no DNS CIM objects. Enumerate once, then
        # match by index; missing rows mean no configured DNS, while a discovery
        # failure still stops activation rather than hiding a competing resolver.
        $dnsInterfaces = @(Get-DnsClientServerAddress)
        foreach ($other in @(Get-NetAdapter -IncludeHidden | Where-Object { $_.Status -eq 'Up' -and $_.ifIndex -ne $adapter.ifIndex })) {
            if (@($dnsInterfaces | Where-Object { $_.InterfaceIndex -eq $other.ifIndex -and $_.ServerAddresses.Count -gt 0 }).Count -gt 0) { throw 'Another active adapter has DNS servers; multiple adapters/VPNs are unsupported.' }
        }
        if ($request.action -eq 'guard') {
            @{ ok = $true } | ConvertTo-Json -Compress
            exit 0
        }
        $profile = @($profiles | Where-Object { $_.InterfaceIndex -eq $adapter.ifIndex })[0]
        $networkId = (Machine-Id) + ':' + ([guid]$profile.InstanceID).ToString('D')
        $states = @(foreach ($family in @('ipv4','ipv6')) {
            $binding = if ($family -eq 'ipv4') { 'ms_tcpip' } else { 'ms_tcpip6' }
            if (!(Get-NetAdapterBinding -Name $adapter.Name -ComponentID $binding | Where-Object { $_.Name -ceq $adapter.Name }).Enabled) { continue }
            $setting = Read-Setting $adapter $family
            foreach ($server in @((Get-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -AddressFamily $family).ServerAddresses)) {
                if ([Net.IPAddress]::IsLoopback([Net.IPAddress]::Parse($server))) { throw 'Existing local DNS software is unsupported.' }
            }
            @{ target = @{ interfaceId = $request.interfaceId; networkId = $networkId; family = $family }; setting = $setting }
        })
        @{ states = $states } | ConvertTo-Json -Depth 10 -Compress
        exit 0
    }
    if ($request.action -notin @('read','cas')) { throw 'Unknown adapter operation.' }
    if ($request.target.family -notin @('ipv4','ipv6')) { throw 'Invalid address family.' }
    $adapter = Resolve-Target $request.target
    if ($null -eq $adapter) {
        if ($request.action -eq 'cas') { throw 'Adapter or network identity changed.' }
        @{ setting = $null } | ConvertTo-Json -Compress
        exit 0
    }
    $setting = Read-Setting $adapter $request.target.family
    if ($request.action -eq 'cas') {
        Require-Administrator
        if (!(Same-Setting $setting $request.expected)) { throw 'DNS was edited outside NAAB; it will not be overwritten.' }
        # Resolve stable identity and read the expected setting immediately before
        # handing the specific family's CIM object to the Windows setter.
        $adapter = Resolve-Target $request.target
        if ($null -eq $adapter -or !(Same-Setting (Read-Setting $adapter $request.target.family) $request.expected)) { throw 'DNS target changed before write.' }
        $dns = Get-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -AddressFamily $request.target.family
        if ($request.replacement.mode -eq 'automatic') {
            Reset-DnsFamily $adapter $request.target.family
        } elseif ($request.replacement.mode -eq 'static') {
            $dns | Set-DnsClientServerAddress -ServerAddresses ([string[]]$request.replacement.servers) -Confirm:$false
        } else { throw 'Invalid replacement setting.' }
        # OS configuration APIs do not provide atomic compare-and-set. The
        # controller additionally reads back and retains conflicts for recovery.
    }
    @{ setting = $setting } | ConvertTo-Json -Depth 8 -Compress
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
