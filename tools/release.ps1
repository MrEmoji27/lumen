param()
$ErrorActionPreference = "Stop"

# 1. clean tree
$porcelain = git status --porcelain --untracked-files=no
if ($porcelain) {
    Write-Error "commit first"
    exit 1
}

# 2. changelog line
$version = (Get-Content Cargo.toml | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1) -replace '^version\s*=\s*"','' -replace '"\s*$',''
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

# 5. rename release exes
foreach ($name in @("lumen.exe","lumen-agent.exe")) {
    $p = Join-Path "target\release" $name
    if (Test-Path $p) {
        $prev = Join-Path "target\release" ($name -replace '\.exe$','.prev.exe')
        if (Test-Path $prev) { Remove-Item $prev -Force -ErrorAction SilentlyContinue }
        try {
            Rename-Item $p ($name -replace '\.exe$','.prev.exe') -ErrorAction Stop
        } catch {
            $stamp = Get-Date -Format "HHmm"
            $alt = Join-Path "target\release" ($name -replace '\.exe$',"") + ".prev-$stamp.exe"
            Rename-Item $p (Split-Path $alt -Leaf)
        }
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