@echo off
setlocal
if "%~1"=="" exit /b 2
set "WHPX_TEST_OUTPUT=%~f1"
for /f "usebackq delims=" %%I in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "WHPX_TEST_VS=%%I"
if not defined WHPX_TEST_VS exit /b 2
call "%WHPX_TEST_VS%\Common7\Tools\VsDevCmd.bat" -no_logo -arch=x64 -host_arch=x64
if errorlevel 1 exit /b 1
if not exist "%WHPX_TEST_OUTPUT%" mkdir "%WHPX_TEST_OUTPUT%"
cd /d "%WHPX_TEST_OUTPUT%"
if errorlevel 1 exit /b 1
cl /nologo /std:c11 /W4 /WX /O2 /TC "%~dp0probe.c" /link WinHvPlatform.lib /OUT:probe.exe
exit /b %errorlevel%
