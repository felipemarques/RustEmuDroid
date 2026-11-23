@echo off
setlocal
cd /d "%~dp0"
powershell -NoProfile -Command "$exe = [IO.Path]::GetFullPath('.\target\integrado\release\rustemudroid.exe'); $app = Get-Process -Name rustemudroid -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1; if ($app) { (New-Object -ComObject WScript.Shell).AppActivate($app.Id) | Out-Null; exit 0 }; exit 1"
if not errorlevel 1 exit /b 0
cargo build --locked --release --features ui --target-dir target/integrado
if errorlevel 1 (
    pause
    exit /b 1
)
powershell -NoProfile -Command "Start-Process -FilePath '.\target\integrado\release\rustemudroid.exe' -ArgumentList 'ui' -WorkingDirectory (Get-Location).Path -WindowStyle Hidden"
