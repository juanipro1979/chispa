@echo off
rem Doble clic para instalar Chispa (sin permisos de administrador)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
pause
