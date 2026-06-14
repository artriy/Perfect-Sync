# Build Perfect-Sync into a testable Windows app.
# Usage:  ./scripts/build.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "Installing JS deps..." -ForegroundColor Cyan
pnpm install

Write-Host "Building release app + NSIS installer..." -ForegroundColor Cyan
pnpm tauri build --bundles nsis

$rel = Join-Path $root "target\release"
$portable = Join-Path $rel "app.exe"
$installer = Get-ChildItem (Join-Path $rel "bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1

Write-Host ""
Write-Host "Done." -ForegroundColor Green
if (Test-Path $portable)  { Write-Host "Portable exe : $portable" }
if ($installer)           { Write-Host "Installer    : $($installer.FullName)" }
Write-Host "Tip: run the portable exe to test without installing."
