[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargoPath = if ($cargoCommand) { $cargoCommand.Source } else {
    Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
}
if (-not (Test-Path -LiteralPath $cargoPath)) {
    throw 'Cargo was not found. Install Rust or restart the terminal after installation.'
}
$originalProcessPath = $env:PATH
$env:PATH = (Split-Path -Parent $cargoPath) + ';' + $env:PATH
Push-Location $projectRoot
try {
    # Native tray tests use an interactive Windows desktop and synthetic peers only.
    $checks = @(
        @('fmt', '--all', '--', '--check'),
        @('clippy', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings'),
        # Native interaction tests share the desktop; use the documented fixed
        # serial run. Default-order instability remains tracked as TD-14.
        @('test', '--all-targets', '--all-features', '--locked', '--', '--test-threads=1'),
        @('build', '--release', '--bin', 'quotarail', '--bin', 'quota_probe', '--locked')
    )
    foreach ($check in $checks) {
        & $cargoPath @check
        if ($LASTEXITCODE -ne 0) { throw ('Cargo check failed: ' + ($check -join ' ')) }
    }
} finally {
    Pop-Location
    $env:PATH = $originalProcessPath
}
