[CmdletBinding()]
param()

# P1's driver always runs all targets/features, including P2 layout and native widget
# tests, and builds both release binaries. Native UI tests require an interactive desktop.
& (Join-Path $PSScriptRoot 'Test-P1.ps1')
