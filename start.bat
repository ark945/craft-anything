@echo off
chcp 65001 >nul
echo 正在啟動 Craft Anything 桌面版 (v0.4.0)...
start "" "%~dp0dist\craft-anything-0.4.0-windows-x64-portable\craft-anything.exe" %*

