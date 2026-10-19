param()
$ErrorActionPreference = "Stop"

# 1. clean tree
$porcelain = git status --porcelain --untracked-files=no
if ($porcelain) {
    Write-Error "commit first"
    exit 1
}

# 2. changelog line
$base = (Get-Content Cargo.toml | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1) -replace '^version\s*=\s*"','' -replace '"\s*$',''
$rel = (Get-Content Cargo.toml | Where-Object { $_ -match '^release\s*=' } | Select-Object -First 1) -replace '^release\s*=\s*','' -replace '\s*$',''
$version = if ($rel) { "$base.$rel" } else { $base }
$changelog = Get-Content docs/changelog.md -Raw
if (-not ($changelog -match ("## " + [regex]::Escape($version) + "\s*[-—]"))) {
    Write-Error "add a changelog line for $version"
    exit 1
}

# 3. tests
cargo test -j 2
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# 4. smoke
cargo test -j 2 --test agent_smoke -- --ignored
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# 5. keep the old exes, named by the version they carry: lumen-0.6.7.2.exe, lumen-agent-0.6.7.2.exe.
#    The version is read from the stamp inside the exe ("0.6.7.2 (8e9c131 2026-10-19)", "+dev" if
#    it was built from uncommitted work). An exe from before the stamp is named by its date.
#    A running exe can be renamed on Windows, so a running Lumen or agent door is no obstacle.
foreach ($name in @("lumen.exe","lumen-agent.exe")) {
    $p = Join-Path "target\release" $name
    if (Test-Path $p) {
        $text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes((Resolve-Path $p)))
        $m = [regex]::Match($text, '(\d+\.\d+\.\d+(?:\.\d+)?) \([0-9a-f]{7}[^)]*?( \+dev)?\)')
        $old = if ($m.Success) { $m.Groups[1].Value + $(if ($m.Groups[2].Success) { "-dev" } else { "" }) } else { (Get-Item $p).LastWriteTime.ToString("yyyy-MM-dd-HHmm") }
        $stem = $name -replace '\.exe$',''
        $dest = "$stem-$old.exe"
        if (Test-Path (Join-Path "target\release" $dest)) { $dest = "$stem-$old-" + (Get-Date -Format "HHmm") + ".exe" }
        Rename-Item $p $dest
        Write-Host "kept the old $name as $dest"
    }
}

# 6. release build. embed_resource sometimes cannot find RC.EXE; a second try passes.
cargo build --release -j 2
if ($LASTEXITCODE -ne 0) {
    Write-Host "release build failed; trying once more"
    cargo build --release -j 2
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# 7. print version with LUMEN_DATA temp
$tmp = Join-Path $env:TEMP ("lumen-release-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
$env:LUMEN_DATA = $tmp
& "target\release\lumen-agent.exe" --version
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue