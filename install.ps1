# RustCode Installer for Windows
# Usage: irm https://raw.githubusercontent.com/LHagfoss/rustcode/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = "LHagfoss/rustcode"
$AssetName = "rustcode-windows-x86_64.zip"

function Write-Info ($Message) {
    Write-Host "==> " -ForegroundColor Cyan -NoNewline
    Write-Host $Message
}

function Write-Success ($Message) {
    Write-Host "==> " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Write-Warn ($Message) {
    Write-Host "Warning: " -ForegroundColor Yellow -NoNewline
    Write-Host $Message
}

function Write-Err ($Message) {
    Write-Host "Error: " -ForegroundColor Red -NoNewline
    Write-Host $Message
    exit 1
}

Write-Info "Checking system architecture..."
$Arch = $env:PROCESSOR_ARCHITECTURE
if ($Arch -ne "AMD64" -and $Arch -ne "ARM64") {
    Write-Warn "Detected architecture $Arch. RustCode provides x86_64 binaries for Windows."
}

Write-Info "Fetching latest release information from GitHub..."
$LatestTag = $null

try {
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "rustcode-installer" }
    $LatestTag = $Release.tag_name
} catch {
    # Fallback if GitHub API is rate-limited
    try {
        $Req = [System.Net.WebRequest]::Create("https://github.com/$Repo/releases/latest")
        $Req.AllowAutoRedirect = $true
        $Resp = $Req.GetResponse()
        $LatestUrl = $Resp.ResponseUri.AbsoluteUri
        $LatestTag = $LatestUrl.Substring($LatestUrl.LastIndexOf('/') + 1)
        $Resp.Close()
    } catch {
        Write-Err "Could not retrieve the latest release information."
    }
}

if (-not $LatestTag) {
    Write-Err "Failed to determine latest release tag."
}

$DownloadUrl = "https://github.com/$Repo/releases/download/$LatestTag/$AssetName"
Write-Info "Downloading RustCode $LatestTag ($AssetName)..."

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("rustcode_install_" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TempDir $AssetName
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing

    Write-Info "Extracting archive..."
    Expand-Archive -Path $ZipPath -DestinationPath $TempDir -Force

    $ExeFile = Get-ChildItem -Path $TempDir -Filter "rustcode*.exe" -Recurse | Select-Object -First 1
    if (-not $ExeFile) {
        Write-Err "Could not find rustcode.exe inside downloaded archive."
    }

    $InstallDir = Join-Path $env:USERPROFILE ".rustcode\bin"
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $DestExe = Join-Path $InstallDir "rustcode.exe"
    Write-Info "Installing to $DestExe..."
    Copy-Item -Path $ExeFile.FullName -Destination $DestExe -Force

    Write-Success "RustCode $LatestTag successfully installed to $DestExe!"

    # Check and update PATH environment variable
    $UserPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
    $PathParts = ($UserPath -split ';') | Where-Object { $_ -ne "" }

    if ($PathParts -notcontains $InstallDir) {
        Write-Info "Adding $InstallDir to your user PATH..."
        $NewUserPath = "$UserPath;$InstallDir"
        [Environment]::SetEnvironmentVariable("Path", $NewUserPath, [EnvironmentVariableTarget]::User)
        $env:Path = "$env:Path;$InstallDir"
        Write-Success "Updated PATH. You may need to restart existing terminal windows for PATH to take effect."
    }

    Write-Host ""
    Write-Host "Run 'rustcode' to start pair programming!" -ForegroundColor Cyan
} finally {
    if (Test-Path $TempDir) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
