[CmdletBinding()]
param()
# All synthetic tests, including isolated current-user registry fixtures.
# Never enables the real QuotaRail startup entry or queries a live account.
& (Join-Path $PSScriptRoot 'Test-P1.ps1')
