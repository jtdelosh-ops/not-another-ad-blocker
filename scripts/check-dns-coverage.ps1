param(
    [Parameter(Mandatory = $true)]
    [string]$Config,
    [string]$BaselineReport,
    [int]$MinimumEffectiveBlockRules = 0,
    [ValidateRange(0, 100)]
    [int]$MaximumEffectiveDropPercent = 25,
    [switch]$FailOnSuppression
)

$ErrorActionPreference = 'Stop'

$arguments = @(
    'run', '--locked', '--manifest-path', 'companion/Cargo.toml',
    '--bin', 'naab-dns-dev', '--', '--config', $Config, '--check'
)
$jsonLines = & cargo @arguments 2>$null
if ($LASTEXITCODE -ne 0) {
    throw "DNS coverage compilation failed for $Config"
}

try {
    $report = ($jsonLines -join "`n") | ConvertFrom-Json
} catch {
    throw "DNS coverage command did not return valid JSON: $($_.Exception.Message)"
}

function Get-Count([object]$Object, [string]$Name, [string]$Label) {
    $value = $Object.$Name
    if ($null -eq $value -or $value -is [bool] -or $value -isnot [ValueType]) {
        throw "$Label.$Name must be a nonnegative integer"
    }
    $number = [double]$value
    if ([double]::IsNaN($number) -or [double]::IsInfinity($number) -or
        $number -lt 0 -or $number -ne [math]::Truncate($number)) {
        throw "$Label.$Name must be a nonnegative integer"
    }
    return [int64]$number
}

$coverage = $report.compilation.coverage
if ($null -eq $coverage -or $null -eq $coverage.effectiveBlockRules -or
    $null -eq $coverage.candidateBlockRules -or $null -eq $coverage.inputLines) {
    throw 'DNS coverage report is missing required coverage fields'
}
if ($coverage.listBlocksSuppressed -isnot [bool]) {
    throw 'DNS coverage report is missing a Boolean listBlocksSuppressed field'
}
$inputLines = Get-Count $coverage 'inputLines' 'coverage'
$candidateBlockRules = Get-Count $coverage 'candidateBlockRules' 'coverage'
$effectiveBlockRules = Get-Count $coverage 'effectiveBlockRules' 'coverage'
$listAllowRules = Get-Count $coverage 'listAllowRules' 'coverage'
$unsupportedLines = Get-Count $coverage 'unsupportedLines' 'coverage'
if ($effectiveBlockRules -lt $MinimumEffectiveBlockRules) {
    throw "Effective DNS block rules ($effectiveBlockRules) are below the required minimum ($MinimumEffectiveBlockRules)"
}
if ($FailOnSuppression -and $coverage.listBlocksSuppressed) {
    throw 'DNS list blocking is safety-suppressed'
}

if ($BaselineReport) {
    $baseline = Get-Content -Raw -LiteralPath $BaselineReport | ConvertFrom-Json
    if ($null -eq $baseline.compilation -or $null -eq $baseline.compilation.coverage -or
        $null -eq $baseline.compilation.coverage.effectiveBlockRules) {
        throw 'Baseline report is missing compilation.coverage.effectiveBlockRules'
    }
    $baselineEffective = Get-Count $baseline.compilation.coverage 'effectiveBlockRules' 'baseline coverage'
    if ($baselineEffective -le 0) {
        throw 'Baseline report must contain a positive effectiveBlockRules value'
    }
    $drop = (($baselineEffective - $effectiveBlockRules) * 100.0) / $baselineEffective
    if ($drop -gt $MaximumEffectiveDropPercent) {
        $displayDrop = [math]::Round($drop, 1)
        throw "Effective DNS block rules dropped by $displayDrop% from baseline; maximum allowed is $MaximumEffectiveDropPercent%"
    }
}

[pscustomobject]@{
    Config = $Config
    InputLines = $inputLines
    CandidateBlockRules = $candidateBlockRules
    EffectiveBlockRules = $effectiveBlockRules
    ListAllowRules = $listAllowRules
    UnsupportedLines = $unsupportedLines
    ListBlocksSuppressed = $coverage.listBlocksSuppressed
    Status = 'PASS'
}
