# Compila con prioridad baja y 2 hilos para no molestar mientras se juega.
# Uso: .\build-low.ps1 build --release -p chispa-core
$p = Start-Process cargo -ArgumentList (@($args) + @('-j', '2', '--color', 'never')) -NoNewWindow -PassThru `
    -RedirectStandardError "$PSScriptRoot\target\build-low.log"
try { $p.PriorityClass = 'Idle' } catch {}
$p.WaitForExit()
Get-Content "$PSScriptRoot\target\build-low.log" | Select-String -Pattern '^(error|warning)|-->|^\s+\||Finished|test result' -Context 0,3 | ForEach-Object { $_.ToString() } | Select-Object -First 120
"EXIT: $($p.ExitCode)"
