# Desinstala Chispa. Tus clips y grabaciones (Videos\Chispa) NO se borran.
$dst = Join-Path $env:LOCALAPPDATA "Programs\Chispa"
Get-Process chispa -ErrorAction SilentlyContinue | Stop-Process -Force
foreach ($lnk in @(
    (Join-Path ([Environment]::GetFolderPath("Programs")) "Chispa.lnk"),
    (Join-Path ([Environment]::GetFolderPath("Desktop")) "Chispa.lnk")
)) { Remove-Item $lnk -ErrorAction SilentlyContinue }

# Se borra desde otro proceso porque este script vive dentro de la carpeta
Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile", "-Command", "Start-Sleep 2; Remove-Item -LiteralPath '$dst' -Recurse -Force"
Write-Host "Chispa desinstalada. Tus clips siguen en Videos\Chispa."
Write-Host "Ajustes: %APPDATA%\Chispa · Claves: Administrador de credenciales de Windows (entradas 'Chispa')."
