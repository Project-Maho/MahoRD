$ErrorActionPreference = "Continue"
$exe = "C:\erd\clients\rust\target\release\maho-host.exe"
Write-Output "=== stopping legacy task/process ==="
schtasks /end /tn "erd-host-run" 2>&1 | Out-String | Write-Output
Stop-Process -Name maho-host -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Write-Output "=== installing service ==="
& $exe --install-service 2>&1 | Out-String | Write-Output
Write-Output "INSTALL_EXIT=$LASTEXITCODE"
Start-Sleep -Seconds 3
Write-Output "=== sc query ==="
sc.exe query MahoRDHost 2>&1 | Out-String | Write-Output
Write-Output "=== workers ==="
Get-CimInstance Win32_Process -Filter "Name='maho-host.exe'" | ForEach-Object { $s=0; $null=[void]0; Write-Output ("PID=" + $_.ProcessId + " CMD=" + $_.CommandLine) }
Get-Process maho-host -ErrorAction SilentlyContinue | Select-Object Id,SessionId | Format-Table -AutoSize | Out-String | Write-Output
