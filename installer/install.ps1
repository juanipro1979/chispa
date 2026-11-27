# Instala Chispa para el usuario actual (no pide permisos de administrador).
# Copia la app a %LOCALAPPDATA%\Programs\Chispa y crea accesos directos en el menú Inicio y el escritorio.
$ErrorActionPreference = "Stop"
$src = $PSScriptRoot
$dst = Join-Path $env:LOCALAPPDATA "Programs\Chispa"

Get-Process chispa -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $dst | Out-Null
Get-ChildItem $src | Where-Object { $_.Name -notin @("install.ps1", "Instalar Chispa.cmd") } |
    Copy-Item -Destination $dst -Recurse -Force

$shell = New-Object -ComObject WScript.Shell
$targets = @(
    (Join-Path ([Environment]::GetFolderPath("Programs")) "Chispa.lnk"),
    (Join-Path ([Environment]::GetFolderPath("Desktop")) "Chispa.lnk")
)
foreach ($lnk in $targets) {
    $s = $shell.CreateShortcut($lnk)
    $s.TargetPath = Join-Path $dst "chispa.exe"
    $s.WorkingDirectory = $dst
    $s.Description = "Chispa: grabá, clipeá y transmití en PCs flojas"
    $s.Save()
}

Write-Host "Chispa quedó instalada en $dst" -ForegroundColor Green
Write-Host "Abrila desde el menú Inicio o el escritorio. Para desinstalar: $dst\uninstall.ps1"
Start-Process (Join-Path $dst "chispa.exe") -WorkingDirectory $dst
