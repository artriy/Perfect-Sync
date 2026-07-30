# Build Perfect Sync into a testable Windows app.
# Usage:  ./scripts/build.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "Installing JS deps..." -ForegroundColor Cyan
pnpm install

$keyPath = Join-Path $root ".tauri\perfect-sync.key"
$passwordPath = Join-Path $root ".tauri\perfect-sync.password"
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    if (-not (Test-Path $keyPath)) {
        throw "Missing updater signing key: $keyPath"
    }
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyPath -Raw
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    if (-not (Test-Path $passwordPath)) {
        throw "Missing updater signing password: $passwordPath"
    }
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content $passwordPath -Raw).Trim()
}

Write-Host "Building signed release app + NSIS installer..." -ForegroundColor Cyan
pnpm tauri build --bundles nsis

$rel = Join-Path $root "target\release"
$version = (Get-Content (Join-Path $root "package.json") -Raw | ConvertFrom-Json).version
$portable = Join-Path $rel "app.exe"
$installer = Get-ChildItem (Join-Path $rel "bundle\nsis") -Filter "*_$($version)_*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
$signature = Get-ChildItem (Join-Path $rel "bundle\nsis") -Filter "*_$($version)_*-setup.exe.sig" -ErrorAction SilentlyContinue | Select-Object -First 1

Write-Host ""
Write-Host "Done." -ForegroundColor Green
if (Test-Path $portable)  { Write-Host "Portable exe : $portable" }
if ($installer)           { Write-Host "Installer    : $($installer.FullName)" }
if ($signature)           { Write-Host "Signature    : $($signature.FullName)" }
Write-Host "Tip: run the portable exe to test without installing."
