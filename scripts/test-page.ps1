param([ValidateRange(0, 65535)][int]$Port = 8765)
$ErrorActionPreference = 'Stop'
$naabNodeCommand = Get-Command node -ErrorAction SilentlyContinue
if ($naabNodeCommand) { $naabNodePath = $naabNodeCommand.Source }
else {
    $naabNodePath = Join-Path $env:USERPROFILE '.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe'
    if (-not (Test-Path -LiteralPath $naabNodePath)) { throw 'Node.js was not found. Install Node.js 22 or newer, then run this script again.' }
}
& $naabNodePath (Join-Path $PSScriptRoot 'test-page.mjs') --port $Port
