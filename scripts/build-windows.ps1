param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    foreach ($tool in @('npm.cmd', 'cargo', 'rustup')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Missing required tool: $tool" }
    }
    & npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    & rustup target add x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Installing Windows Rust target failed' }
    $buildArgs = @('run', 'tauri', '--', 'build', '--target', 'x86_64-pc-windows-msvc', '--bundles', 'nsis')
    if ($DebugBuild) { $buildArgs += '--debug' }
    & npm.cmd @buildArgs
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    $profile = if ($DebugBuild) { 'debug' } else { 'release' }
    Write-Host "Installer: src-tauri/target/x86_64-pc-windows-msvc/$profile/bundle/nsis/"
    Write-Host "Executable: src-tauri/target/x86_64-pc-windows-msvc/$profile/linger.exe"
} finally {
    Pop-Location
}
