<#
.SYNOPSIS
  Install Pelago on Windows (x86-64 or ARM64).
.DESCRIPTION
  irm https://pelagolang.github.io/install.ps1 | iex
  & ([scriptblock]::Create((irm https://pelagolang.github.io/install.ps1))) -Channel nightly

  Checks the SHA-256 of the archive against the release's SHA256SUMS, and the Ed25519 signature of
  SHA256SUMS when the release has one and OpenSSL 3 is installed (Git for Windows ships one). Nothing is
  installed on a failure. Windows binaries are not Authenticode-signed yet; a file fetched by this script
  carries no download mark, so SmartScreen does not prompt, but the checksum is what you can rely on.
.PARAMETER Channel
  stable (default), beta, rc or nightly.
.PARAMETER Version
  An exact release, for example 0.1.0.
.PARAMETER Dir
  Where to put pelago.exe (default %LOCALAPPDATA%\pelago\bin).
.PARAMETER AllowUnsigned
  Go on when the signature cannot be checked.
.PARAMETER NoPath
  Do not add the folder to your user PATH.
#>
param(
  [ValidateSet('stable', 'beta', 'rc', 'nightly')][string]$Channel = 'stable',
  [string]$Version = '',
  [string]$Dir = (Join-Path $env:LOCALAPPDATA 'pelago\bin'),
  [switch]$AllowUnsigned,
  [switch]$NoPath
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'pelagolang/pelago'
$Api = if ($env:PELAGO_UPDATE_API) { $env:PELAGO_UPDATE_API } else { "https://api.github.com/repos/$Repo" }
$Download = if ($env:PELAGO_DOWNLOAD_BASE) { $env:PELAGO_DOWNLOAD_BASE } else { "https://github.com/$Repo/releases/download" }
# The release public key (PEM), the same as docs/release/pelago-release.pub.
$PubKey = @'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAZQly4/VA1yqhmSBaaxjG6i/t48crC3Ed9QfFYPFbXQE=
-----END PUBLIC KEY-----
'@

# `throw`, not `exit`: with `irm | iex` an `exit` would close the user's terminal window.
function Fail($m) { throw "pelago install: $m" }

$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
  'X64' { 'x86_64' }
  'Arm64' { 'aarch64' }
  default { Fail "unsupported processor $_" }
}
$target = "$arch-windows"
$headers = @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'pelago-install' }

# Highest version of a list of tags, ordering X.Y.Z numbers (a pre-release suffix ranks its own N).
function Sort-Tags($tags) {
  $tags | Sort-Object {
    $m = [regex]::Match($_, '^v(\d+)\.(\d+)\.(\d+)(?:-(?:beta|rc)\.(\d+))?$')
    $n = if ($m.Groups[4].Success) { [int]$m.Groups[4].Value } else { 0 }
    '{0:D6}.{1:D6}.{2:D6}.{3:D6}' -f [int]$m.Groups[1].Value, [int]$m.Groups[2].Value, [int]$m.Groups[3].Value, $n
  }
}

$releases = Invoke-RestMethod -Headers $headers -Uri "$Api/releases?per_page=100"
$tags = @($releases | ForEach-Object { $_.tag_name })

if ($Version) {
  $tag = 'v' + $Version.TrimStart('v')
  if ($tags -notcontains $tag) { Fail "no release $Version" }
  $nameVersion = $tag.Substring(1)
} elseif ($Channel -eq 'nightly') {
  $tag = 'nightly'
  $rel = Invoke-RestMethod -Headers $headers -Uri "$Api/releases/tags/nightly"
  $assets = @()
  $page = 1
  do {
    $chunk = @(Invoke-RestMethod -Headers $headers -Uri "$($rel.assets_url)?per_page=100&page=$page")
    $assets += $chunk
    $page++
  } while ($chunk.Count -ge 100)
  $versions = @($assets | ForEach-Object {
      if ($_.name -match "^pelago-(.+)-$target\.zip$") { $Matches[1] }
    })
  if ($versions.Count -eq 0) { Fail "no nightly build for $target yet" }
  # YYYYMMDD sorts as text, so the newest nightly is the last one.
  $nameVersion = ($versions | Sort-Object {
      $p = $_ -split '-nightly\.'
      $b = $p[0].Split('.')
      '{0:D6}.{1:D6}.{2:D6}-{3}' -f [int]$b[0], [int]$b[1], [int]$b[2], $p[1]
    } | Select-Object -Last 1)
} else {
  $pattern = switch ($Channel) {
    'stable' { '^v\d+\.\d+\.\d+$' }
    'beta' { '^v\d+\.\d+\.\d+-beta\.\d+$' }
    'rc' { '^v\d+\.\d+\.\d+-rc\.\d+$' }
  }
  $tag = Sort-Tags @($tags | Where-Object { $_ -match $pattern }) | Select-Object -Last 1
  if (-not $tag) { Fail "no $Channel release yet" }
  $nameVersion = $tag.Substring(1)
}

$archive = "pelago-$nameVersion-$target.zip"
$sums = if ($tag -eq 'nightly') { "SHA256SUMS-$nameVersion" } else { 'SHA256SUMS' }
Write-Host "installing pelago $nameVersion ($target)"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("pelago-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Invoke-WebRequest -Headers @{ 'User-Agent' = 'pelago-install' } -Uri "$Download/$tag/$archive" -OutFile (Join-Path $tmp $archive)
  Invoke-WebRequest -Headers @{ 'User-Agent' = 'pelago-install' } -Uri "$Download/$tag/$sums" -OutFile (Join-Path $tmp $sums)

  $line = Get-Content (Join-Path $tmp $sums) | Where-Object { $_ -match "  $([regex]::Escape($archive))$" } | Select-Object -First 1
  if (-not $line) { Fail "$archive is not in $sums" }
  $expected = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $archive)).Hash.ToLower()
  if ($expected -ne $actual) { Fail "checksum mismatch for $archive" }
  Write-Host 'checksum ok'

  $sigOk = $false
  try {
    Invoke-WebRequest -Headers @{ 'User-Agent' = 'pelago-install' } -Uri "$Download/$tag/$sums.sig" -OutFile (Join-Path $tmp "$sums.sig")
    $sigOk = $true
  } catch { }
  $ssl = Get-Command openssl -ErrorAction SilentlyContinue
  if (-not $ssl) {
    $git = 'C:\Program Files\Git\usr\bin\openssl.exe'
    if (Test-Path $git) { $ssl = $git }
  }
  if ($sigOk -and $PubKey -and $ssl) {
    Set-Content -Path (Join-Path $tmp 'key.pub') -Value $PubKey
    & $ssl pkeyutl -verify -rawin -pubin -inkey (Join-Path $tmp 'key.pub') -in (Join-Path $tmp $sums) -sigfile (Join-Path $tmp "$sums.sig") *> $null
    if ($LASTEXITCODE -ne 0) { Fail "the signature on $sums is not valid" }
    Write-Host 'signature ok'
  } elseif ($AllowUnsigned) {
    Write-Host 'signature not checked: going on because of -AllowUnsigned'
  } elseif (-not $sigOk) {
    Fail 'this release has no signature; pass -AllowUnsigned to go on with the checksum only'
  } else {
    Fail 'cannot check the signature here (no release key in this script, or no OpenSSL); pass -AllowUnsigned to go on with the checksum only'
  }

  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  $bin = Join-Path $tmp "pelago-$nameVersion-$target\pelago.exe"
  if (-not (Test-Path $bin)) { Fail 'the archive has no pelago.exe' }
  $said = & $bin --version
  if (-not ($said -like "pelago $nameVersion*")) { Fail "the downloaded binary reports '$said', expected $nameVersion" }

  New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  $dest = Join-Path $Dir 'pelago.exe'
  if (Test-Path $dest) {
    # A running pelago.exe cannot be overwritten, but it can be renamed.
    Remove-Item "$dest.old" -ErrorAction SilentlyContinue
    Move-Item -Force $dest "$dest.old"
  }
  Copy-Item $bin $dest
  Set-Content -Path (Join-Path $Dir 'pelago.channel') -Value $Channel
  Write-Host "installed $dest"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

if (-not $NoPath) {
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($user -split ';') -notcontains $Dir) {
    [Environment]::SetEnvironmentVariable('Path', ($user.TrimEnd(';') + ';' + $Dir), 'User')
    Write-Host "added $Dir to your user PATH; open a new terminal to use it"
  }
}
