param(
    [string]$Name = 'ads.example.test',
    [string]$Server = '127.0.0.1',
    [int]$Port = 5354
)

$ErrorActionPreference = 'Stop'

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
# Transaction ID 0x4e41, standard recursive query, one question.
$packet.AddRange([byte[]](0x4e, 0x41, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00))
$labels = Convert-NameToDnsLabels $Name
$packet.AddRange([byte[]]$labels)
$packet.AddRange([byte[]](0x00, 0x01, 0x00, 0x01)) # A / IN

$udp = [Net.Sockets.UdpClient]::new()
$udp.Client.ReceiveTimeout = 3000
try {
    [void]$udp.Send($packet.ToArray(), $packet.Count, $Server, $Port)
    $remote = [Net.IPEndPoint]::new([Net.IPAddress]::Any, 0)
    $response = $udp.Receive([ref]$remote)
} catch {
    throw "No DNS response from ${Server}:${Port}. Confirm naab-dns-dev is running and the port is correct. $($_.Exception.Message)"
} finally {
    $udp.Dispose()
}

if ($response.Length -lt 12) { throw 'DNS response was shorter than its header' }
$id = ($response[0] -shl 8) -bor $response[1]
$rcode = $response[3] -band 0x0f
$rcodeName = switch ($rcode) {
    0 { 'NOERROR' }
    2 { 'SERVFAIL' }
    3 { 'NXDOMAIN' }
    5 { 'REFUSED' }
    default { "RCODE_$rcode" }
}

[pscustomobject]@{
    Name = $Name
    Server = $Server
    Port = $Port
    TransactionId = ('0x{0:x4}' -f $id)
    ResponseCode = $rcodeName
    Bytes = $response.Length
}
