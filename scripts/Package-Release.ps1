[CmdletBinding()]
param(
    [string]$OutputDirectory,
    [string]$ExpectedTag
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $projectRoot 'artifacts/releases' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
Push-Location $projectRoot
try {
    $metadataText = & cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read locked dependency metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.id -eq $metadata.resolve.root }
    $tag = 'v' + $package.version
    if ($ExpectedTag -and $ExpectedTag -cne $tag) { throw "Tag must match Cargo version: $tag" }
    $name = "QuotaRail-$tag-windows-x64"
    $zipPath = Join-Path $OutputDirectory ($name + '.zip')
    if (Test-Path -LiteralPath $zipPath) { throw "Output already exists: $zipPath" }
    if (-not (Test-Path -LiteralPath "docs/releases/$tag.md")) { throw 'Missing release notes.' }

    # Separate target directory: never reuse a test-support executable or lock a running app.
    $buildDirectory = Join-Path $projectRoot 'artifacts/release-build'
    & cargo build --release --locked --target x86_64-pc-windows-msvc --target-dir $buildDirectory --bin quotarail --bin quota_probe
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $stage = Join-Path $OutputDirectory ($name + '-stage-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    foreach ($binary in @('quotarail.exe', 'quota_probe.exe')) {
        Copy-Item -LiteralPath (Join-Path $buildDirectory "x86_64-pc-windows-msvc/release/$binary") -Destination $stage
    }
    foreach ($file in @('README.md', 'LICENSE', 'CHANGELOG.md', 'CONTRIBUTING.md', 'PRIVACY.md', 'SECURITY.md')) {
        Copy-Item -LiteralPath $file -Destination $stage
    }
    New-Item -ItemType Directory -Path (Join-Path $stage 'docs/images'), (Join-Path $stage 'assets') | Out-Null
    Copy-Item -LiteralPath 'docs/compatibility.md' -Destination (Join-Path $stage 'docs')
    Copy-Item -LiteralPath 'docs/releasing.md' -Destination (Join-Path $stage 'docs')
    Copy-Item -LiteralPath 'docs/images/quota-rail.png' -Destination (Join-Path $stage 'docs/images')
    Copy-Item -LiteralPath 'assets/quotarail.png' -Destination (Join-Path $stage 'assets')
    Copy-Item -LiteralPath "docs/releases/$tag.md" -Destination (Join-Path $stage 'RELEASE-NOTES.md')

    $licenses = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $licenses | Out-Null
    $notice = @('# Third-party notices', '', 'Dependency licenses apply independently of the QuotaRail MIT license.', '', '| Crate | Version | SPDX license | License texts |', '| --- | --- | --- | --- |')
    $resolvedIds = @($metadata.resolve.nodes | ForEach-Object { $_.id })
    foreach ($dependency in ($metadata.packages | Where-Object { $_.id -ne $package.id -and $_.id -in $resolvedIds } | Sort-Object name, version)) {
        if (-not $dependency.license) { throw "Missing license metadata: $($dependency.name)" }
        $texts = @(Get-ChildItem -LiteralPath (Split-Path -Parent $dependency.manifest_path) -File |
            Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' })
        if ($texts.Count -eq 0) { throw "Missing license texts: $($dependency.name)" }
        $folder = "$($dependency.name)-$($dependency.version)"
        $destination = Join-Path $licenses $folder
        New-Item -ItemType Directory -Path $destination | Out-Null
        $texts | Copy-Item -Destination $destination
        $notice += "| $($dependency.name) | $($dependency.version) | $($dependency.license) | [texts](licenses/$folder/) |"
    }
    Set-Content -LiteralPath (Join-Path $stage 'THIRD-PARTY-NOTICES.md') -Value $notice -Encoding utf8
    $rustVersion = & rustc --version
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record Rust version.' }
    $commit = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot record source revision.' }
    @("version=$($package.version)", 'target=x86_64-pc-windows-msvc', "compiler=$rustVersion", "commit=$commit", 'features=default (no test-support)') |
        Set-Content -LiteralPath (Join-Path $stage 'BUILD-INFO.txt') -Encoding utf8
    # Compress only this explicit staging tree, never the working directory.
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zipPath -CompressionLevel Optimal
    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name.zip" | Set-Content -LiteralPath (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding ascii
    Write-Output "Package: $zipPath"
    Write-Output "SHA256: $hash"
} finally {
    Pop-Location
}
