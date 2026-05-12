# Sign one or more Windows artifacts (.exe / .msi) with signtool.
#
# Usage (CI):
#   $env:WINDOWS_PFX_BASE64   = "<base64-encoded .pfx>"
#   $env:WINDOWS_PFX_PASSWORD = "<cert password>"
#   pwsh scripts/sign-windows.ps1 -Paths @("path/to/Said.msi", "path/to/Said-setup.exe")
#
# Usage (local dev with a personal cert):
#   $env:DEV_CERT_PATH        = "C:/path/to/dev.pfx"
#   $env:WINDOWS_PFX_PASSWORD = "<cert password>"
#   pwsh scripts/sign-windows.ps1 -Paths @("path/to/Said.msi")
#
# Notes:
#   * Always SHA-256 digest + timestamp via DigiCert's free RFC 3161 server.
#     SignPath sub-CA certs use the same flow.
#   * `signtool verify /pa` is run after signing as a sanity check.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string[]]$Paths
)

$ErrorActionPreference = 'Stop'

function Resolve-CertPath {
    if ($env:WINDOWS_PFX_BASE64) {
        # CI path: write the base64-encoded .pfx to a temp file the
        # PowerShell session will clean up at exit.
        $tmp = [System.IO.Path]::GetTempFileName()
        [System.IO.File]::WriteAllBytes(
            $tmp,
            [Convert]::FromBase64String($env:WINDOWS_PFX_BASE64)
        )
        return $tmp
    }
    if ($env:DEV_CERT_PATH -and (Test-Path $env:DEV_CERT_PATH)) {
        return $env:DEV_CERT_PATH
    }
    throw 'No signing certificate. Set WINDOWS_PFX_BASE64 (CI) or DEV_CERT_PATH (local).'
}

function Find-Signtool {
    $candidates = @(
        'signtool.exe',
        "${env:ProgramFiles(x86)}/Windows Kits/10/bin/10.0.22621.0/x64/signtool.exe",
        "${env:ProgramFiles(x86)}/Windows Kits/10/bin/10.0.22000.0/x64/signtool.exe",
        "${env:ProgramFiles(x86)}/Windows Kits/10/bin/10.0.19041.0/x64/signtool.exe"
    )
    foreach ($c in $candidates) {
        if (Get-Command $c -ErrorAction SilentlyContinue) {
            return (Get-Command $c).Source
        }
        if (Test-Path $c) {
            return $c
        }
    }
    throw 'signtool.exe not found. Install the Windows 10 SDK or add signtool to PATH.'
}

if (-not $env:WINDOWS_PFX_PASSWORD) {
    throw 'WINDOWS_PFX_PASSWORD is not set.'
}

$cert = Resolve-CertPath
$signtool = Find-Signtool

try {
    foreach ($p in $Paths) {
        if (-not (Test-Path $p)) {
            Write-Warning "Skipping missing path: $p"
            continue
        }
        Write-Host "Signing $p"
        & $signtool sign `
            /f $cert `
            /p $env:WINDOWS_PFX_PASSWORD `
            /tr 'http://timestamp.digicert.com' `
            /td sha256 `
            /fd sha256 `
            /d 'Said' `
            /du 'https://github.com/EMIAC-org/Said' `
            $p
        if ($LASTEXITCODE -ne 0) {
            throw "signtool sign failed for $p (exit $LASTEXITCODE)"
        }

        & $signtool verify /pa $p
        if ($LASTEXITCODE -ne 0) {
            throw "signtool verify failed for $p (exit $LASTEXITCODE)"
        }
    }
} finally {
    if ($env:WINDOWS_PFX_BASE64 -and (Test-Path $cert)) {
        Remove-Item -Force $cert
    }
}

Write-Host 'All artifacts signed + verified.'
