# Installs nexo on Windows from the GitHub releases, no Rust needed:
#
#   irm https://raw.githubusercontent.com/Qwizi/nexo-cli/main/install.ps1 | iex
#
# Settings (environment): NEXO_VERSION (tag, default latest), NEXO_INSTALL_DIR
# (default %LOCALAPPDATA%\Programs\nexo), NEXO_URL (panel to sign in to),
# NEXO_NO_LOGIN=1, GH_TOKEN (only while the repository is private).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'Qwizi/nexo-cli'
$api = "https://api.github.com/repos/$repo"
$target = 'x86_64-pc-windows-msvc'

$token = if ($env:GH_TOKEN) { $env:GH_TOKEN } elseif ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } else { $null }
if (-not $token -and (Get-Command gh -ErrorAction SilentlyContinue)) {
    $token = (gh auth token 2>$null)
}
$headers = @{ Accept = 'application/vnd.github+json' }
if ($token) { $headers.Authorization = "Bearer $token" }

$release = if ($env:NEXO_VERSION) {
    Invoke-RestMethod -Headers $headers "$api/releases/tags/$($env:NEXO_VERSION)"
} else {
    try { Invoke-RestMethod -Headers $headers "$api/releases/latest" }
    catch { throw 'Nie mogę pobrać informacji o wydaniu (prywatne repo? ustaw GH_TOKEN albo zaloguj gh).' }
}
$tag = $release.tag_name
$archive = "nexo-$tag-$target.zip"

function Get-Asset($name, $path) {
    $asset = $release.assets | Where-Object { $_.name -eq $name } | Select-Object -First 1
    if (-not $asset) { throw "Wydanie $tag nie ma pliku $name." }
    $h = $headers.Clone()
    $h.Accept = 'application/octet-stream'
    Invoke-WebRequest -Headers $h -OutFile $path "$api/releases/assets/$($asset.id)"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("nexo-" + [guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Write-Host "Pobieram nexo $tag ($target)..."
    Get-Asset $archive (Join-Path $tmp $archive)
    Get-Asset 'SHA256SUMS' (Join-Path $tmp 'SHA256SUMS')
    $expected = (Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match " $([regex]::Escape($archive))$" }) -split ' ' | Select-Object -First 1
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $archive)).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) { throw 'Suma kontrolna się nie zgadza.' }

    Expand-Archive (Join-Path $tmp $archive) -DestinationPath $tmp
    $dir = if ($env:NEXO_INSTALL_DIR) { $env:NEXO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\nexo' }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item -Force (Join-Path $tmp "nexo-$tag-$target\nexo.exe") (Join-Path $dir 'nexo.exe')
    Write-Host "Zainstalowano $dir\nexo.exe"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" } else { '' }) + $dir), 'User')
    Write-Host "Dodałem $dir do PATH (działa w nowym oknie terminala)."
}
$env:Path = "$dir;$env:Path"

if ($env:NEXO_NO_LOGIN -eq '1') { return }
$url = $env:NEXO_URL
if (-not $url) {
    $url = Read-Host 'Adres panelu CMS do zalogowania (np. https://cms.twoja-domena.pl, Enter = później)'
}
if ($url) {
    & (Join-Path $dir 'nexo.exe') login --url $url
} else {
    Write-Host 'Zaloguj się później: nexo login'
}
