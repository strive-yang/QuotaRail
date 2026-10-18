[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    # Compile every desktop test, but do not simulate Explorer input on hosted CI.
    $checks = @(
        @('fmt', '--all', '--', '--check'),
        @('clippy', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings'),
        @('test', '--all-targets', '--all-features', '--locked', '--no-run'),
        @('test', '--lib', '--all-features', '--locked', '--', '--test-threads=1'),
        @('test', '--all-features', '--locked',
          '--test', 'discovery', '--test', 'framing', '--test', 'lifecycle',
          '--test', 'normalization', '--test', 'p1_cache', '--test', 'p1_state',
          '--test', 'p2_layout', '--test', 'p3_preferences', '--test', 'probe_cli',
          '--', '--test-threads=1'),
        @('test', '--all-features', '--locked', '--test', 'p3_settings',
          'desktop_binary_embeds_standard_user_and_per_monitor_manifest',
          '--', '--exact', '--test-threads=1')
    )
    foreach ($check in $checks) {
        & cargo @check
        if ($LASTEXITCODE -ne 0) { throw ('Cargo check failed: ' + ($check -join ' ')) }
    }
} finally {
    Pop-Location
}
