[CmdletBinding()]
param(
    [switch]$Live,
    [ValidateRange(0, 86400)]
    [int]$ObserveSeconds = 0,
    [string]$CodexPath,
    [string]$CodexHome
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargoCommand) {
    $cargoPath = $cargoCommand.Source
} else {
    $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
}
if (-not (Test-Path -LiteralPath $cargoPath)) {
    throw 'Cargo was not found. Install Rust or restart the terminal after installation.'
}
if ($ObserveSeconds -gt 0 -and -not $Live) {
    throw 'Resource observation requires -Live. Offline tests never query a real account.'
}

# This changes only this process environment; no persistent PATH or Rust settings.
$originalProcessPath = $env:PATH
$env:PATH = (Split-Path -Parent $cargoPath) + ';' + $env:PATH
Push-Location $projectRoot
try {
    $checks = @(
        @('fmt', '--all', '--', '--check'),
        @('clippy', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings'),
        @('test', '--all-targets', '--all-features', '--locked'),
        @('build', '--release', '--bin', 'quota_probe', '--locked')
    )
    foreach ($check in $checks) {
        & $cargoPath @check
        if ($LASTEXITCODE -ne 0) { throw ('Cargo check failed: ' + ($check -join ' ')) }
    }
    if ($Live) {
        $probeArguments = @('--json')
        if ($CodexPath) { $probeArguments += @('--codex', $CodexPath) }
        if ($CodexHome) { $probeArguments += @('--codex-home', $CodexHome) }
        if ($ObserveSeconds -gt 0) {
            $evidenceDirectory = Join-Path $projectRoot 'evidence\local'
            New-Item -ItemType Directory -Force -Path $evidenceDirectory | Out-Null
            $metricsPath = Join-Path $evidenceDirectory ('p0-' + [guid]::NewGuid().ToString('N') + '.jsonl')
            $probeArguments += @('--observe-seconds', [string]$ObserveSeconds, '--metrics', $metricsPath)
            Write-Output ('Resource counters only: ' + $metricsPath)
        }
        & (Join-Path $projectRoot 'target\release\quota_probe.exe') @probeArguments
        if ($LASTEXITCODE -ne 0) { throw 'The live probe failed; see its sanitized error output.' }
    }
} finally {
    Pop-Location
    $env:PATH = $originalProcessPath
}
