param([string]$Label, [bool]$InProc, [int]$Port, [string]$Url = "https://www.youtube.com/watch?v=aqz-KE-bpKQ", [int]$Secs = 60)
$W = Split-Path -Parent $PSScriptRoot
$D = Join-Path $env:TEMP "lumen_$Label"
Remove-Item $D -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $D | Out-Null
$ip = if ($InProc) { "true" } else { "false" }
'{"agent": true, "agent_port": ' + $Port + ', "gpu_in_process": ' + $ip + '}' | Set-Content "$D\settings.json"
$env:LUMEN_DATA = $D; $env:LUMEN_HEADLESS = "1"; $env:LUMEN_AGENT_IDLE = "6"
$env:LUMEN_ARGS = "--autoplay-policy=no-user-gesture-required"
$p = Start-Process "$W\target\debug\lumen.exe" -PassThru -WindowStyle Hidden
Start-Sleep 6
$A = "$W\target\debug\lumen-agent.exe"

function TreeIds($root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine
  $ids = @($root); $queue = @($root)
  while ($queue.Count) { $c = $queue[0]; $queue = $queue[1..($queue.Count)]; foreach ($x in $all | Where-Object { $_.ParentProcessId -eq $c }) { $ids += $x.ProcessId; $queue += $x.ProcessId } }
  ,$ids
}
function CpuSecs($ids) { (Get-Process -Id $ids -ErrorAction SilentlyContinue | Measure-Object -Property CPU -Sum).Sum }

& $A open "url=$Url" | Select-Object -First 2
Start-Sleep 12
$js = "const v=document.querySelector('video');if(!v)return null;v.muted=true;v.play();return 1"
& $A eval tab=2 "script=$js" | Out-Null
$pin = "const p=document.getElementById('movie_player');if(!p||!p.setPlaybackQualityRange)return null;p.setPlaybackQualityRange('hd720','hd720');return p.getPlaybackQuality()"
& $A eval tab=2 "script=$pin" | Out-Null
Start-Sleep 14
$q = "const v=document.querySelector('video');if(!v)return null;const q=v.getVideoPlaybackQuality();return JSON.stringify({t:v.currentTime,total:q.totalVideoFrames,dropped:q.droppedVideoFrames,w:v.videoWidth,h:v.videoHeight,paused:v.paused,ended:v.ended,rate:v.playbackRate,ad:!!document.querySelector('.ad-showing'),codecs:(()=>{try{return document.getElementById('movie_player').getStatsForNerds().codecs}catch(e){return String(e)}})()})"
$q0 = & $A eval tab=2 "script=$q"
$ids = TreeIds $p.Id
$c0 = CpuSecs $ids
$t0 = Get-Date
# Hardware decode: GPU Engine counters for any VideoDecode engine of this tree, sampled mid-run.
Start-Sleep ([int]($Secs / 2))
$gpu = $null
try {
  $samples = (Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples
  $mine = $samples | Where-Object { ($ids | ForEach-Object { "pid_$_" }) -contains (($_.InstanceName -split '_luid')[0]) }
  $gpu = (($mine | Where-Object { $_.CookedValue -gt 0 } | ForEach-Object { ($_.InstanceName -replace '.*(engtype_\w+).*','$1') + '=' + [math]::Round($_.CookedValue,1) }) -join ' ')
  $gpu += ' | ALL VideoDecode>0: ' + ((($samples | Where-Object { $_.InstanceName -match 'VideoDecode' -and $_.CookedValue -gt 0 } | ForEach-Object { ($_.InstanceName -split '_luid')[0] + '=' + [math]::Round($_.CookedValue,1) }) -join ' '))
  $gpu += ' | engines seen: ' + ((($samples | ForEach-Object { $_.InstanceName -replace '.*(engtype_\w+).*','$1' } | Sort-Object -Unique) -join ','))
} catch { $gpu = "n/a: $($_.Exception.Message)" }
Start-Sleep ([int]($Secs - ($Secs / 2)))
$q1 = & $A eval tab=2 "script=$q"
$mc = "const q=String.fromCharCode(34);const r=await navigator.mediaCapabilities.decodingInfo({type:'media-source',video:{contentType:'video/mp4; codecs='+q+'avc1.640028'+q,width:1280,height:720,bitrate:3000000,framerate:60}});return JSON.stringify({smooth:r.smooth,powerEfficient:r.powerEfficient})"
"mediaCapabilities h264: " + (& $A eval tab=2 "script=$mc")
$ids = TreeIds $p.Id
$c1 = CpuSecs $ids
$secs = ((Get-Date) - $t0).TotalSeconds
$ps = Get-Process -Id $ids -ErrorAction SilentlyContinue
$ws = [math]::Round(($ps | Measure-Object WorkingSet64 -Sum).Sum / 1MB, 1)
$pv = [math]::Round(($ps | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB, 1)
$cmd = Get-CimInstance Win32_Process | Where-Object { $ids -contains $_.ProcessId }
$gpuProcs = @($cmd | Where-Object { $_.CommandLine -match '--type=gpu-process' }).Count
"== $Label (gpu_in_process=$InProc)"
"start:  $q0"
"end:    $q1"
"CPU: {0:N1}% of one core over {1:N0}s   WS={2} MB  Private={3} MB  processes={4}  gpu-process-count={5}  VideoDecode GPU util={6}" -f ((($c1 - $c0) / $secs) * 100), $secs, $ws, $pv, $ids.Count, $gpuProcs, $gpu
& $A close tab=2 | Out-Null
Start-Sleep 12
if (!$p.HasExited) { Stop-Process -Id $p.Id -Force }
