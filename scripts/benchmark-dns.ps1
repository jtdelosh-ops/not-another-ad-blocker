param(
    [string]$Name = 'ads.example.test',
    [int]$Count = 100,
    [string]$Server = '127.0.0.1',
    [int]$Port = 5354,
    [ValidateSet('NOERROR', 'SERVFAIL', 'NXDOMAIN', 'REFUSED')]
    [string]$ExpectedResponseCode = 'NXDOMAIN'
)

$ErrorActionPreference = 'Stop'
if ($Count -lt 1 -or $Count -gt 10000) { throw 'Count must be between 1 and 10000' }

function Convert-NameToDnsLabels([string]$Value) {
    $trimmed = $Value.TrimEnd('.')
    if ([string]::IsNullOrWhiteSpace($trimmed)) { throw 'Name must not be empty' }
    $bytes = [System.Collections.Generic.List[byte]]::new()
    foreach ($label in $trimmed.Split('.')) {
        $labelBytes = [Text.Encoding]::ASCII.GetBytes($label)
        if ($labelBytes.Length -lt 1 -or $labelBytes.Length -gt 63) { throw "Invalid DNS label: $label" }
        $bytes.Add([byte]$labelBytes.Length)
        $bytes.AddRange($labelBytes)
    }
    $bytes.Add(0)
    return $bytes
}

$packet = [System.Collections.Generic.List[byte]]::new()
$packet.AddRange([byte[]](0x4e, 0x41, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00))
$packet.AddRange([byte[]](Convert-NameToDnsLabels $Name))
$packet.AddRange([byte[]](0x00, 0x01, 0x00, 0x01))

$udp = [Net.Sockets.UdpClient]::new()
$udp.Client.ReceiveTimeout = 3000
$udp.Connect($Server, $Port)
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$failures = 0
$lastCode = $null
try {
    for ($index = 0; $index -lt $Count; $index++) {
        $packet[0] = [byte](($index -shr 8) -band 0xff)
        $packet[1] = [byte]($index -band 0xff)
        try {
            [void]$udp.Send($packet.ToArray(), $packet.Count)
            $remote = [Net.IPEndPoint]::new([Net.IPAddress]::Any, 0)
            $response = $udp.Receive([ref]$remote)
            if ($response.Length -lt 12) { throw 'DNS response was shorter than its header' }
            $responseId = ([int]$response[0] -shl 8) -bor [int]$response[1]
            $queryId = ([int]$packet[0] -shl 8) -bor [int]$packet[1]
            if ($responseId -ne $queryId -or ($response[2] -band 0x80) -eq 0) {
                throw 'DNS response did not match the query transaction or response flag'
            }
            $rcode = $response[3] -band 0x0f
            $lastCode = switch ($rcode) {
                0 { 'NOERROR' }
                2 { 'SERVFAIL' }
                3 { 'NXDOMAIN' }
                5 { 'REFUSED' }
                default { "RCODE_$rcode" }
            }
            if ($ExpectedResponseCode -and $lastCode -ne $ExpectedResponseCode) { throw "Expected $ExpectedResponseCode but received $lastCode" }
        } catch {
            $failures++
        }
    }
} finally {
    $stopwatch.Stop()
    $udp.Dispose()
}

if ($failures -gt 0) { throw "$failures of $Count DNS queries failed" }

[pscustomobject]@{
    Name = $Name
    Server = $Server
    Port = $Port
    Queries = $Count
    ResponseCode = $lastCode
    ElapsedMilliseconds = [math]::Round($stopwatch.Elapsed.TotalMilliseconds, 2)
    AverageMilliseconds = [math]::Round($stopwatch.Elapsed.TotalMilliseconds / $Count, 2)
}
