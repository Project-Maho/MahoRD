$ErrorActionPreference = "Continue"
sc.exe stop MahoRDHost | Out-Null
Start-Sleep -Seconds 3
Get-Process maho-host -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
cd C:\erd\clients\rust
cargo build --release -p maho-host 2>&1 | Select-Object -Last 5 | ForEach-Object { Write-Output $_ }
Write-Output ("BUILD_EXIT=" + $LASTEXITCODE)
if ($LASTEXITCODE -ne 0) { exit 1 }
Remove-Item C:\ProgramData\MahoRD\service.log -ErrorAction SilentlyContinue
Remove-Item C:\ProgramData\MahoRD\input-desktop.txt -ErrorAction SilentlyContinue
& C:\erd\clients\rust\target\release\maho-host.exe --install-service 2>&1 | ForEach-Object { Write-Output $_ }
Start-Sleep -Seconds 12
sc.exe query MahoRDHost | Select-String STATE | ForEach-Object { Write-Output $_.Line.Trim() }
Get-CimInstance Win32_Process -Filter "Name = 'maho-host.exe'" | ForEach-Object { Write-Output ('PROC PID=' + $_.ProcessId + ' SESSION=' + $_.SessionId) }
Write-Output '=== DESKTOP HINT ==='
if (Test-Path C:\ProgramData\MahoRD\input-desktop.txt) { Write-Output ('HINT=' + (Get-Content C:\ProgramData\MahoRD\input-desktop.txt -Raw).Trim()) } else { Write-Output 'NO_HINT_FILE' }
Write-Output '=== SUPERVISOR ==='
Get-Content C:\ProgramData\MahoRD\service.log -Tail 5 | ForEach-Object { Write-Output $_ }
