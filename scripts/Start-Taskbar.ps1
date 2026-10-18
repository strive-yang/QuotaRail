[CmdletBinding()]
param([ValidateRange(0,1024)][int]$LeftSpaceDip=240)
$ErrorActionPreference='Stop'
$projectRoot=Split-Path -Parent $PSScriptRoot
$executable=Join-Path $projectRoot 'target\release\quotarail.exe'
if(-not (Test-Path -LiteralPath $executable)) {
    throw 'Build first: cargo build --release --bin quotarail --locked'
}
Start-Process -FilePath $executable -ArgumentList @('--taskbar','--taskbar-left-space',"$LeftSpaceDip") -WindowStyle Hidden
