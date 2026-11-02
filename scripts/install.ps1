# RustCode Installer for Windows
# Usage: irm https://rustcode.lhagfoss.com/install.ps1 | iex
#        & ([scriptblock]::Create((irm https://rustcode.lhagfoss.com/install.ps1))) -NoApp
#
# Installs the rustcode CLI to %USERPROFILE%\.rustcode\bin and the RustCode
# desktop app with its per-user installer (Start menu entry, no admin rights).
# Every download is checked against the release SHA256SUMS.
#
#   -NoApp   install only the CLI      (or $env:RUSTCODE_NO_APP = "1")
#   -NoCli   install only the app      (or $env:RUSTCODE_NO_CLI = "1")
#   $env:RUSTCODE_INSTALL_ARTIFACTS = "<dir>" installs from a directory of
#   release assets and their SHA256SUMS instead of GitHub (docs/install.md).

param(
    [switch]$NoApp,
    [switch]$NoCli
)

$ErrorActionPreference = 'Stop'

$Repo = "LHagfoss/rustcode"
$AssetName = "rustcode-windows-x86_64.zip"
$InstallCli = -not ($NoCli -or $env:RUSTCODE_NO_CLI -eq "1")
$InstallApp = -not ($NoApp -or $env:RUSTCODE_NO_APP -eq "1")
$Artifacts = $env:RUSTCODE_INSTALL_ARTIFACTS

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

if (-not $InstallCli -and -not $InstallApp) {
    Write-Err "Nothing to install: -NoApp and -NoCli together."
}

Write-Info "Checking system architecture..."
$Arch = $env:PROCESSOR_ARCHITECTURE
if ($Arch -eq "ARM64") {
    Write-Warn "Windows ARM64 detected; using the x86_64 RustCode binary through Windows emulation."
} elseif ($Arch -ne "AMD64") {
    Write-Warn "Detected architecture $Arch. RustCode provides x86_64 binaries for Windows."
}

$LatestTag = $null
if ($Artifacts) {
    if (-not (Test-Path -PathType Container $Artifacts)) {
        Write-Err "RUSTCODE_INSTALL_ARTIFACTS=$Artifacts is not a directory."
    }
    $Artifacts = (Resolve-Path $Artifacts).Path
    $LatestTag = "(local artifacts in $Artifacts)"
    Write-Info "Installing from $Artifacts instead of GitHub."
} else {
    Write-Info "Fetching latest release information from GitHub..."
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
}

# A release asset by name: copied from RUSTCODE_INSTALL_ARTIFACTS, else downloaded.
function Get-Asset ($Name, $Destination) {
    if ($Artifacts) {
        $Source = Join-Path $Artifacts $Name
        if (-not (Test-Path -PathType Leaf $Source)) { throw "$Name is not in $Artifacts" }
        Copy-Item -Path $Source -Destination $Destination -Force
    } else {
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$LatestTag/$Name" -OutFile $Destination -UseBasicParsing
    }
}

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("rustcode_install_" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    $ManifestName = "SHA256SUMS"
    $ManifestPath = Join-Path $TempDir $ManifestName
    $ManifestDownloaded = $true
    try {
        Get-Asset $ManifestName $ManifestPath
    } catch {
        $ManifestDownloaded = $false
    }

    # Names of every SHA256SUMS entry, for assets whose name carries the version.
    function Get-ManifestNames {
        if (-not $ManifestDownloaded) { return @() }
        @(Get-Content -Path $ManifestPath | ForEach-Object {
            $Parts = $_.Trim() -split '\s+'
            if ($Parts.Count -ge 2) { $Parts[1].TrimStart('*') }
        })
    }

    # Fetches an asset and checks it against SHA256SUMS; returns its path.
    function Get-VerifiedAsset ($Name) {
        $Path = Join-Path $TempDir $Name
        Write-Info "Downloading $Name..."
        Get-Asset $Name $Path
        Write-Info "Verifying $Name against $ManifestName..."
        if (-not $ManifestDownloaded) {
            # v0.36.0 predates the published SHA256SUMS asset. Keep this exact,
            # one-release mapping so existing installers remain verifiable.
            if ($LatestTag -eq "v0.36.0" -and $Name -eq "rustcode-windows-x86_64.zip") {
                $ExpectedHash = "dea6a42383dea5f04baa36f78e373b7faf0db303c084882e3dbb3d8d5d4a3786"
                Write-Warn "$ManifestName is unavailable for $LatestTag; using the embedded official one-release migration checksum."
            } else {
                Write-Err "Could not download $ManifestName for $LatestTag; refusing to install an unverified download."
            }
        } else {
            $ManifestEntries = @(
                Get-Content -Path $ManifestPath | ForEach-Object {
                    $Parts = $_.Trim() -split '\s+'
                    if ($Parts.Count -lt 2) {
                        return
                    }
                    $EntryName = $Parts[1].TrimStart('*')
                    if ($EntryName -ceq $Name) {
                        [PSCustomObject]@{
                            FieldCount = $Parts.Count
                            Hash = $Parts[0]
                        }
                    }
                }
            )
            if ($ManifestEntries.Count -ne 1) {
                Write-Err "$ManifestName has no single valid entry for $Name."
            }
            if ($ManifestEntries[0].FieldCount -ne 2) {
                Write-Err "$ManifestName contains a malformed entry for $Name."
            }
            $ExpectedHash = $ManifestEntries[0].Hash.ToLowerInvariant()
        }
        if ($ExpectedHash -notmatch '^[0-9a-f]{64}$') {
            Write-Err "$ManifestName contains a malformed checksum for $Name."
        }
        $ActualHash = (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLowerInvariant()
        if ($ActualHash -cne $ExpectedHash) {
            Write-Err "SHA-256 mismatch for $Name; refusing to install it."
        }
        return $Path
    }

    if ($InstallCli) {
        $ZipPath = Get-VerifiedAsset $AssetName

        Write-Info "Extracting archive..."
        $CliDir = Join-Path $TempDir "cli"
        Expand-Archive -Path $ZipPath -DestinationPath $CliDir -Force

        $ExeFile = Get-ChildItem -Path $CliDir -Filter "rustcode*.exe" -Recurse | Select-Object -First 1
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
    }

    if ($InstallApp) {
        $SetupName = @(Get-ManifestNames | Where-Object { $_ -cmatch '^RustCode-[0-9][^ ]*-windows-x64-setup\.exe$' })
        if ($SetupName.Count -ne 1) {
            Write-Warn "Release $LatestTag has no desktop app for Windows; skipping it."
        } else {
            $SetupPath = Get-VerifiedAsset $SetupName[0]
            # The per-user install lives in %LOCALAPPDATA%\RustCode; quit a
            # running copy so its files can be replaced.
            $AppDir = Join-Path $env:LOCALAPPDATA "RustCode"
            Get-Process -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -and $_.Path.StartsWith($AppDir + "\", [StringComparison]::OrdinalIgnoreCase) } |
                Stop-Process -Force -ErrorAction SilentlyContinue
            Write-Info "Installing the RustCode app for this user..."
            $Setup = Start-Process -FilePath $SetupPath -ArgumentList "/S" -Wait -PassThru
            if ($Setup.ExitCode -ne 0) {
                Write-Err "The RustCode app installer failed (exit code $($Setup.ExitCode))."
            }
            Write-Success "RustCode app installed. Open it from the Start menu."
        }
    }

    if ($InstallCli) {
        Write-Host ""
        Write-Host "Run 'rustcode' to start pair programming!" -ForegroundColor Cyan
    }
} finally {
    if (Test-Path $TempDir) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
