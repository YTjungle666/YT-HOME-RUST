param(
    [ValidateSet("amd64", "arm64")]
    [string]$Architecture = "amd64",
    [string]$SingBoxVersion = "1.13.5"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path $PSScriptRoot -Parent
Set-Location $RepoRoot

$TargetTriple = if ($Architecture -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$AssetArch = if ($Architecture -eq "arm64") { "arm64" } else { "amd64" }
$OutputDir = Join-Path $PSScriptRoot "dist\$Architecture"
$WebDir = Join-Path $OutputDir "web"
$MigrationsDir = Join-Path $OutputDir "migrations"

Write-Host "Building YT HOME RUST for Windows ($Architecture)..." -ForegroundColor Green

rustup target add $TargetTriple | Out-Null

Push-Location frontend
npm ci
npm run build
Pop-Location

cargo build --release -p app --target $TargetTriple

if (Test-Path $OutputDir) {
    Remove-Item $OutputDir -Recurse -Force
}
New-Item -ItemType Directory -Path $WebDir -Force | Out-Null
New-Item -ItemType Directory -Path $MigrationsDir -Force | Out-Null

Copy-Item "target\$TargetTriple\release\app.exe" (Join-Path $OutputDir "sui.exe")
Copy-Item "frontend\dist\*" $WebDir -Recurse -Force
Copy-Item "crates\infra-db\migrations\*" $MigrationsDir -Recurse -Force

$ZipPath = Join-Path $env:TEMP "sing-box-$SingBoxVersion-windows-$AssetArch.zip"
$ExtractRoot = Join-Path $env:TEMP "sing-box-$SingBoxVersion-windows-$AssetArch"
$ExtractDir = Join-Path $ExtractRoot "sing-box-$SingBoxVersion-windows-$AssetArch"
$DownloadUrl = "https://github.com/SagerNet/sing-box/releases/download/v$SingBoxVersion/sing-box-$SingBoxVersion-windows-$AssetArch.zip"

if (Test-Path $ZipPath) {
    Remove-Item $ZipPath -Force
}
if (Test-Path $ExtractRoot) {
    Remove-Item $ExtractRoot -Recurse -Force
}

Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath
Expand-Archive -Path $ZipPath -DestinationPath $ExtractRoot -Force

Copy-Item (Join-Path $ExtractDir "sing-box.exe") (Join-Path $OutputDir "sing-box.exe")
if (Test-Path (Join-Path $ExtractDir "libcronet.dll")) {
    Copy-Item (Join-Path $ExtractDir "libcronet.dll") (Join-Path $OutputDir "libcronet.dll")
}

Copy-Item (Join-Path $PSScriptRoot "install-windows.bat") $OutputDir
Copy-Item (Join-Path $PSScriptRoot "s-ui-windows.bat") $OutputDir
Copy-Item (Join-Path $PSScriptRoot "s-ui-windows.xml") $OutputDir
Copy-Item (Join-Path $PSScriptRoot "uninstall-windows.bat") $OutputDir

Write-Host "Windows bundle is ready at $OutputDir" -ForegroundColor Green
