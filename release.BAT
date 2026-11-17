@echo off
set "TOML_FILE=Cargo.toml"

:: Find the line starting with "version =", split by quotes, and grab the 2nd token
for /f "tokens=2 delims==" %%A in ('findstr /b "version =" "%TOML_FILE%"') do (
    set "RAW_VER=%%A"
)

:: Strip out the double quotes and any leading/trailing spaces
set "VERSION=%RAW_VER:"=%"
set "VERSION=%VERSION: =%"

echo Detected Version: %VERSION%

:: append the variable to command:
powershell -ExecutionPolicy Bypass -File scripts/release.ps1  %VERSION%

pause
