# Authenticode-sign a Windows NSIS installer with the Certum SimplySign certificate.
# Requires SimplySign Desktop logged in (interactive session) so the cert is available
# in the current user certificate store.
#
# Usage:
#   powershell -NoProfile -File scripts/sign-windows-authenticode.ps1 -File path\to\setup.exe
#
# Optional env:
#   CERTUM_CERT_SHA1  - certificate SHA1 thumbprint (default: Open Source Developer Martin Catty)
#   CERTUM_TIMESTAMP  - RFC 3161 timestamp URL (default: http://time.certum.pl)

param(
  [Parameter(Mandatory = $true)]
  [string]$File
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $File)) {
  throw "File not found: $File"
}

$thumbprint = if ($env:CERTUM_CERT_SHA1) {
  ($env:CERTUM_CERT_SHA1 -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
} else {
  '1D3E06AABF499CAA5C3F3FF0D4263B26698F7C7D'
}

$timestamp = if ($env:CERTUM_TIMESTAMP) { $env:CERTUM_TIMESTAMP } else { 'http://time.certum.pl' }

$signtool = $null
$cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
if ($cmd) { $signtool = $cmd.Source }
if (-not $signtool) {
  $signtool = @(
    'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe'
    'C:\Tools\windows-sdk-buildtools\bin\*\x64\signtool.exe'
  ) | ForEach-Object {
    Get-ChildItem $_ -ErrorAction SilentlyContinue
  } | Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName
}

if (-not $signtool) {
  throw 'signtool.exe not found. Install Windows SDK Signing Tools or Microsoft.Windows.SDK.BuildTools.'
}

Write-Host "Using signtool: $signtool"
Write-Host "Signing $File (sha1=$thumbprint)"

# /tr = RFC 3161 timestamp (preferred over legacy /t)
& $signtool sign `
  /sha1 $thumbprint `
  /fd sha256 `
  /td sha256 `
  /tr $timestamp `
  /v `
  $File

if ($LASTEXITCODE -ne 0) {
  throw "signtool failed with exit code $LASTEXITCODE (is SimplySign Desktop logged in?)"
}

& $signtool verify /pa /v $File
if ($LASTEXITCODE -ne 0) {
  throw "signtool verify failed with exit code $LASTEXITCODE"
}

Write-Host 'Authenticode signature OK.'
