# Perf measurement for Lumen. One scenario at a time.
# Usage: pwsh -NoProfile -File tools\measure.ps1 -Tag baseline -Scenario idle
#        add -Stop to kill a leftover test Lumen and its WebView2 tree first.
param(
    [Parameter(Mandatory = $true)][string]$Tag,
    [Parameter(Mandatory = $true)][ValidateSet("idle", "wikipedia", "three")][string]$Scenario,
    [switch]$Stop,
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot | Split-Path -Parent
$exe = Join-Path $root "target\release\lumen.exe"
$agent = Join-Path $root "target\release\lumen-agent.exe"
$out = Join-Path $root "docs\perf-data.csv"

$env:LUMEN_DATA = Join-Path $env:TEMP "lumen-perf-$Tag"

function Get-Mine {
    # lumen.exe of this worktree, plus every WebView2 process whose user data dir is ours.
    Get-CimInstance Win32_Process -Filter "Name='lumen.exe' or Name='msedgewebview2.exe'" |
        Where-Object {
            $_.CommandLine -and (
                ($_.Name -eq "lumen.exe" -and $_.CommandLine -like "*$root*") -or
                ($_.Name -eq "msedgewebview2.exe" -and $_.CommandLine -like "*$env:LUMEN_DATA*")
            )
        }
}

if ($Stop) {
    Get-Mine | ForEach-Object { "killing $($_.ProcessId) $($_.Name)"; Stop-Process -Id $_.ProcessId -Force }
    Start-Sleep -Milliseconds 800
}

function Snap {
    $p = @(Get-Mine)
    $mem = 0; $ws = 0; $cpu = 0.0
    foreach ($x in $p) {
        $mem += $x.WorkingSetSize
        $cpu += (Get-Process -Id $x.ProcessId).TotalProcessorTime.TotalMilliseconds
        $w = Get-CimInstance -ClassName Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($x.ProcessId)" -ErrorAction SilentlyContinue
        if ($w) { $ws += $w.WorkingSetPrivate }
    }
    [pscustomobject]@{ Count = $p.Count; Mem = $mem; WsPrivate = $ws; Cpu = $cpu }
}

function Row($label, $a, $b, $secs) {
    $cpuPct = 0.0
    if ($secs -gt 0) { $cpuPct = [math]::Round(($b.Cpu - $a.Cpu) / 10.0 / $secs, 2) }
    $s = "$Tag,$Scenario,$label,$($b.Count),$([math]::Round($b.Mem/1MB,1)),$([math]::Round($b.WsPrivate/1MB,1)),$cpuPct"
    Write-Output $s
    if (-not (Test-Path $out)) { Add-Content -LiteralPath $out "tag,scenario,label,procs,mem_mb,ws_private_mb,cpu_pct" }
    Add-Content -LiteralPath $out $s
}

if (Test-Path $env:LUMEN_DATA) { Remove-Item -Recurse -Force $env:LUMEN_DATA }
New-Item -ItemType Directory -Path $env:LUMEN_DATA | Out-Null
'{"agent": true, "agent_port": 47698, "adblock": true, "show_memory": true}' |
    Set-Content -LiteralPath (Join-Path $env:LUMEN_DATA "settings.json") -Encoding utf8
$env:LUMEN_AGENT_IDLE = "900"

$proc = $null
if (-not $NoStart) {
    $proc = Start-Process -FilePath $exe -ArgumentList "--headless" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 6
}

$urls = @()
if ($Scenario -eq "wikipedia") { $urls = @("https://en.wikipedia.org/wiki/Lighthouse") }
if ($Scenario -eq "three") { $urls = @("https://en.wikipedia.org/wiki/Lighthouse", "https://news.ycombinator.com", "https://github.com/servo/servo") }

foreach ($u in $urls) {
    & $agent open "url=$u" background=true | Out-Null
    Start-Sleep -Seconds 3
}

Start-Sleep -Seconds 18
$a = Snap
Start-Sleep -Seconds 20
$b = Snap
Row "25s" $a $b 20
Start-Sleep -Seconds 60
$c = Snap
Start-Sleep -Seconds 20
$d = Snap
Row "95s" $c $d 20

Write-Output "-- procs at 95s --"
Get-Mine | ForEach-Object { "$($_.Name) pid=$($_.ProcessId) ws=$([math]::Round($_.WorkingSetSize/1MB,1))MB" }
