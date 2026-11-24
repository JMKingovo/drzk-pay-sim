@echo off
setlocal
cd /d "%~dp0"

echo ============================================================
echo   DRZK Parking Payment Simulator
echo ============================================================
echo   The browser will open automatically.
echo   If it does not, open the address printed below manually.
echo   Close this window to stop the simulator.
echo.

if not exist "drzk-pay-sim.exe" (
  echo [ERROR] drzk-pay-sim.exe not found in this folder.
  echo         Please extract the whole zip again, do not run from inside the zip.
  echo.
  pause
  exit /b 1
)

drzk-pay-sim.exe %*

echo.
echo Simulator exited. Press any key to close this window.
pause >nul
