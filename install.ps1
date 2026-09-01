# Installs the latest Joocode release for Windows. Run from PowerShell:
# irm https://raw.githubusercontent.com/yan-ad/joocode/main/install.ps1 | iex
# To install a version: & .\install.ps1 -Version 0.1.6
[CmdletBinding()]
param(
  [string]$Version = "latest",
  [string]$Repository = $(if ($env:JOOCODE_REPOSITORY) { $env:JOOCODE_REPOSITORY } elseif ($env:JOC_REPOSITORY) { $env:JOC_REPOSITORY } else { "yan-ad/joocode" }),
  [string]$InstallDir = $(if ($env:JOOCODE_INSTALL_DIR) { $env:JOOCODE_INSTALL_DIR } elseif ($env:JOC_INSTALL_DIR) { $env:JOC_INSTALL_DIR } else { Join-Path $HOME ".local\bin" })
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
  throw "joocode installer: $Message"
}

$arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { Fail "32-bit Windows is not supported" }
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [System.Runtime.InteropServices.Architecture]::Arm64) {
  $arch = "aarch64"
}
$target = "${arch}-pc-windows-msvc"
$asset = "joocode-$target.zip"

if ($Version -eq "latest") {
  try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" -Headers @{ Accept = "application/vnd.github+json" }
    $Version = $release.tag_name
  } catch {
    Fail "could not resolve the latest release: $($_.Exception.Message)"
  }
}
if (-not $Version.StartsWith("v")) { $Version = "v$Version" }

$baseUrl = "https://github.com/$Repository/releases/download/$Version"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "joocode-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  Write-Host "Downloading Joocode $Version for $target..."
  $archivePath = Join-Path $tmp $asset
  $checksumsPath = Join-Path $tmp "SHA256SUMS"
  Invoke-WebRequest -Uri "$baseUrl/$asset" -OutFile $archivePath
  Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath

  $checksumLine = Get-Content $checksumsPath | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | Select-Object -First 1
  if (-not $checksumLine) { Fail "checksum for $asset is missing" }
  $expected = ($checksumLine -split '\s+')[0].ToLowerInvariant()
  $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actual -ne $expected) { Fail "checksum verification failed" }

  $extract = Join-Path $tmp "extracted"
  Expand-Archive -Path $archivePath -DestinationPath $extract -Force
  $source = Join-Path $extract "joocode-$target\joocode.exe"
  if (-not (Test-Path -LiteralPath $source)) { Fail "archive did not contain joocode.exe" }

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item -Force -Path $source -Destination (Join-Path $InstallDir "joocode.exe")
  $iconSource = Join-Path $extract "joocode-$target\joocode.ico"
  if (Test-Path -LiteralPath $iconSource) {
    Copy-Item -Force -Path $iconSource -Destination (Join-Path $InstallDir "joocode.ico")
    $programs = [Environment]::GetFolderPath("Programs")
    $shortcutPath = Join-Path $programs "Joocode.lnk"
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = Join-Path $InstallDir "joocode.exe"
    $shortcut.IconLocation = Join-Path $InstallDir "joocode.ico"
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.Save()
  }
  Write-Host "`nJoocode $Version installed to $InstallDir\joocode.exe"

  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $pathItems = @($userPath -split ';' | Where-Object { $_ })
  if ($pathItems -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", "$InstallDir;$userPath", "User")
    Write-Host "Added $InstallDir to your user PATH. Open a new PowerShell window."
  }
  Write-Host "`nNext steps:`n  joocode doctor`n  joocode codex-install`n  joocode"
} finally {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}
