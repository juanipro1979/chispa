# Mide CPU, RAM y GPU de un conjunto de procesos durante N segundos.
# Usa el mismo método para cualquier app, así la comparación es justa.
# Uso: .\bench\measure.ps1 -Names obs64,obs-ffmpeg-mux -Seconds 60 -Warmup 10
param(
    [string[]]$Names,
    [int]$Seconds = 60,
    [int]$Warmup = 10,
    [int]$Interval = 3
)

$threads = [Environment]::ProcessorCount
Start-Sleep -Seconds $Warmup

function Get-Procs { Get-Process -Name $Names -ErrorAction SilentlyContinue }

$cpuSamples = @(); $ramSamples = @(); $gpuSamples = @()
if (-not (Get-Procs)) { Write-Output "NO_PROCESS"; exit 1 }
$prevCpu = (Get-Procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
$prevT = Get-Date
$end = (Get-Date).AddSeconds($Seconds)

while ((Get-Date) -lt $end) {
    Start-Sleep -Seconds $Interval
    $procs = Get-Procs
    if (-not $procs) { break }
    $now = Get-Date
    $cpu = ($procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
    $wall = ($now - $prevT).TotalMilliseconds
    $cpuSamples += (($cpu - $prevCpu) / $wall / $threads * 100)
    $prevCpu = $cpu; $prevT = $now
    $ramSamples += (($procs | Measure-Object -Property WorkingSet64 -Sum).Sum / 1MB)

    # GPU: suma de "Utilization Percentage" de todos los engines de estos PIDs
    $pids = $procs.Id
    try {
        $g = (Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples |
            Where-Object { $p = [regex]::Match($_.InstanceName, 'pid_(\d+)_').Groups[1].Value; $pids -contains [int]$p } |
            Measure-Object -Property CookedValue -Sum
        $gpuSamples += $g.Sum
    } catch {}
}

$o = [ordered]@{
    procesos   = ($Names -join ',')
    muestras   = $cpuSamples.Count
    cpu_prom   = [math]::Round(($cpuSamples | Measure-Object -Average).Average, 1)
    cpu_max    = [math]::Round(($cpuSamples | Measure-Object -Maximum).Maximum, 1)
    ram_prom   = [math]::Round(($ramSamples | Measure-Object -Average).Average, 1)
    ram_max    = [math]::Round(($ramSamples | Measure-Object -Maximum).Maximum, 1)
    gpu_prom   = if ($gpuSamples) { [math]::Round(($gpuSamples | Measure-Object -Average).Average, 1) } else { 'n/d' }
}
[pscustomobject]$o | Format-List
