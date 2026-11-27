# Mide CPU% y RAM de chispa.exe más sus procesos WebView2 hijos (la UI).
param([int]$Seconds = 20, [int]$Interval = 1)

function Get-Tree {
    $root = Get-Process chispa -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $root) { return @() }
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
    $ids = @($root.Id); $added = $true
    while ($added) {
        $added = $false
        foreach ($p in $all) {
            if ($ids -contains $p.ParentProcessId -and -not ($ids -contains $p.ProcessId)) { $ids += $p.ProcessId; $added = $true }
        }
    }
    $ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
}

$threads = [Environment]::ProcessorCount
$ids = (Get-Tree).Id
$cpu = @(); $ramEngine = @(); $ramTotal = @()
$prev = @{}; Get-Process -Id $ids | ForEach-Object { $prev[$_.Id] = $_.TotalProcessorTime.TotalMilliseconds }
$t = Get-Date
for ($i = 0; $i -lt $Seconds; $i += $Interval) {
    Start-Sleep $Interval
    $now = Get-Date; $dt = ($now - $t).TotalMilliseconds; $t = $now
    $procs = Get-Process -Id $ids -ErrorAction SilentlyContinue
    $used = 0
    foreach ($p in $procs) {
        $ms = $p.TotalProcessorTime.TotalMilliseconds
        if ($prev.ContainsKey($p.Id)) { $used += $ms - $prev[$p.Id] }
        $prev[$p.Id] = $ms
    }
    $cpu += 100 * $used / $dt / $threads
    $ramEngine += ($procs | Where-Object ProcessName -eq 'chispa').WorkingSet64 / 1MB
    $ramTotal += ($procs | Measure-Object WorkingSet64 -Sum).Sum / 1MB
}
"procesos: $($ids.Count) | CPU prom {0:N1}% | RAM chispa.exe {1:N0} MB | RAM total (con UI) {2:N0} MB" -f `
    ($cpu | Measure-Object -Average).Average, ($ramEngine | Measure-Object -Average).Average, ($ramTotal | Measure-Object -Average).Average
