# Package Lumen for someone else: dist\lumen-<version>-win64.zip with lumen.exe,
# lumen-agent.exe and README.txt, and the installer when Inno Setup is here. Prints the
# SHA-256 of each.
#
#   tools\package.ps1                      build into target\release (renames running exes
#                                          first, as release.ps1 does), then package
#   tools\package.ps1 -TargetDir target\perf-c   build and package from another target dir
#   tools\package.ps1 -NoBuild             package what is already built
param(
    [string]$TargetDir = "target",
    [switch]$NoBuild
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$base = (Get-Content Cargo.toml | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1) -replace '^version\s*=\s*"','' -replace '"\s*$',''
$rel = (Get-Content Cargo.toml | Where-Object { $_ -match '^release\s*=' } | Select-Object -First 1) -replace '^release\s*=\s*','' -replace '\s*$','' -replace '"',''
$version = if ($rel) { "$base.$rel" } else { $base }
$bin = Join-Path $TargetDir "release"

if (-not $NoBuild) {
    # A running exe cannot be overwritten, but it can be renamed: the person's Lumen keeps
    # running from the old file and the next start gets the new one.
    foreach ($name in @("lumen.exe", "lumen-agent.exe")) {
        $p = Join-Path $bin $name
        if (Test-Path $p) {
            $prev = Join-Path $bin ($name -replace '\.exe$', '.prev.exe')
            if (Test-Path $prev) { Remove-Item $prev -Force -ErrorAction SilentlyContinue }
            try {
                Rename-Item $p (Split-Path $prev -Leaf) -ErrorAction Stop
            } catch {
                $stamp = Get-Date -Format "HHmmss"
                Rename-Item $p ($name -replace '\.exe$', ".prev-$stamp.exe")
            }
        }
    }
    # embed_resource sometimes cannot find RC.EXE; a second try passes.
    cargo build --release --target-dir $TargetDir
    if ($LASTEXITCODE -ne 0) {
        cargo build --release --target-dir $TargetDir
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
}

foreach ($name in @("lumen.exe", "lumen-agent.exe")) {
    if (-not (Test-Path (Join-Path $bin $name))) { Write-Error "$name is not built in $bin" }
}

$dist = "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$stage = Join-Path $dist "lumen-$version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item (Join-Path $bin "lumen.exe"), (Join-Path $bin "lumen-agent.exe") $stage

@"
Lumen $version
==============

A browser for Windows 10 and 11, 64-bit.

Start it
--------
Double-click lumen.exe. There is nothing to install; the folder can live anywhere.

Windows may say "Windows protected your PC" the first time: Lumen is not signed yet. Click
"More info", then "Run anyway". It asks once.

Lumen draws pages with Microsoft WebView2, which Windows 10 and 11 nearly always have. If this
PC does not, Lumen says so and opens Microsoft's download page.

Where your things are
---------------------
Settings, history, bookmarks and the rest: %APPDATA%\Lumen. Delete that folder and Lumen
starts over. Downloads go to your Downloads folder.

AI agents
---------
lumen-agent.exe connects AI agents (Claude Code, opencode, Cursor and others) to Lumen over MCP.
Settings > AI agent turns it on. Without it, ignore the file.

Something wrong?
----------------
Settings > About > Report a problem. It shows exactly what it sends before it sends it.
"@ | Set-Content -Encoding UTF8 (Join-Path $stage "README.txt")

$zip = Join-Path $dist "lumen-$version-win64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Host ("{0}  {1}" -f (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower(), $zip)

# The installer, when Inno Setup 6 is here.
$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc /Q "/DVersion=$version" "/DStage=$((Resolve-Path $stage).Path)" "/O$((Resolve-Path $dist).Path)" "tools\lumen.iss"
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $setup = Join-Path $dist "lumen-$version-setup.exe"
    Write-Host ("{0}  {1}" -f (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower(), $setup)
} else {
    Write-Host "Inno Setup 6 is not installed: no installer. Get it from jrsoftware.org, then run this again."
}
