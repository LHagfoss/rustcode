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
if ($Arch -eq "ARM64") {
    Write-Warn "Windows ARM64 detected; using the x86_64 RustCode binary through Windows emulation."
} elseif ($Arch -ne "AMD64") {
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

    $ManifestName = "SHA256SUMS"
    $ManifestPath = Join-Path $TempDir $ManifestName
    $ManifestUrl = "https://github.com/$Repo/releases/download/$LatestTag/$ManifestName"
    Write-Info "Verifying $AssetName against $ManifestName..."
    $ManifestDownloaded = $true
    try {
        Invoke-WebRequest -Uri $ManifestUrl -OutFile $ManifestPath -UseBasicParsing
    } catch {
        $ManifestDownloaded = $false
    }

    if (-not $ManifestDownloaded) {
        # v0.36.0 predates the published SHA256SUMS asset. Keep this exact,
        # one-release mapping so existing installers remain verifiable.
        if ($LatestTag -eq "v0.36.0") {
            switch ($AssetName) {
                "rustcode-windows-x86_64.zip" {
                    $ExpectedHash = "dea6a42383dea5f04baa36f78e373b7faf0db303c084882e3dbb3d8d5d4a3786"
                }
                default {
                    Write-Err "$ManifestName is unavailable for $LatestTag and no embedded checksum exists for $AssetName."
                }
            }
            Write-Warn "$ManifestName is unavailable for $LatestTag; using the embedded official one-release migration checksum."
        } else {
            Write-Err "Could not download $ManifestName for $LatestTag; refusing to install an unverified archive."
        }
    } else {
        $ManifestEntries = @(
            Get-Content -Path $ManifestPath | ForEach-Object {
                $Parts = $_.Trim() -split '\s+'
                if ($Parts.Count -lt 2) {
                    return
                }
                $Name = $Parts[1].TrimStart('*')
                if ($Name -ceq $AssetName) {
                    [PSCustomObject]@{
                        FieldCount = $Parts.Count
                        Hash = $Parts[0]
                    }
                }
            }
        )
        if ($ManifestEntries.Count -ne 1) {
            Write-Err "$ManifestName has no single valid entry for $AssetName."
        }
        if ($ManifestEntries[0].FieldCount -ne 2) {
            Write-Err "$ManifestName contains a malformed entry for $AssetName."
        }
        $ExpectedHash = $ManifestEntries[0].Hash.ToLowerInvariant()
    }
    if ($ExpectedHash -notmatch '^[0-9a-f]{64}$') {
        Write-Err "$ManifestName contains a malformed checksum for $AssetName."
    }
    $ActualHash = (Get-FileHash -Algorithm SHA256 -Path $ZipPath).Hash.ToLowerInvariant()
    if ($ActualHash -cne $ExpectedHash) {
        Write-Err "SHA-256 mismatch for $AssetName; refusing to install the archive."
    }

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
