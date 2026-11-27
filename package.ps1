# Arma el paquete instalable de Chispa: dist\Chispa-<versión>-windows-x64.zip
# Requiere haber corrido antes:
#   cargo build --release --workspace
#   cargo obs-build build --out-dir target/release
param([string]$Version = "0.5.0")
$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$rel = Join-Path $root "target\release"
$out = Join-Path $root "dist\Chispa"
$zip = Join-Path $root "dist\Chispa-$Version-windows-x64.zip"

if (-not (Test-Path (Join-Path $rel "chispa.exe"))) { throw "Falta target\release\chispa.exe: compilá primero" }
if (-not (Test-Path (Join-Path $rel "obs.dll"))) { throw "Falta el runtime de OBS: corré cargo obs-build build --out-dir target/release" }

if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force $out | Out-Null

# Ejecutable, DLLs del runtime y helpers de OBS (sin símbolos .pdb ni el prototipo de consola)
Get-ChildItem $rel -File | Where-Object {
    $_.Extension -eq ".dll" -or $_.Name -in @("chispa.exe", "obs-ffmpeg-mux.exe", "obs-amf-test.exe", "obs-nvenc-test.exe", "obs-qsv-test.exe")
} | Copy-Item -Destination $out
foreach ($d in "data", "obs-plugins") { Copy-Item (Join-Path $rel $d) (Join-Path $out $d) -Recurse }

Copy-Item (Join-Path $root "installer\install.ps1") $out
Copy-Item (Join-Path $root "installer\uninstall.ps1") $out
Copy-Item (Join-Path $root "installer\Instalar Chispa.cmd") $out
Copy-Item (Join-Path $root "LICENSE") $out

if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$out\*" -DestinationPath $zip -CompressionLevel Optimal
"Listo: $zip ({0:N0} MB)" -f ((Get-Item $zip).Length / 1MB)
