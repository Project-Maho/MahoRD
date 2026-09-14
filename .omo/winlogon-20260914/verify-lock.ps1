$ErrorActionPreference = "Continue"
Get-Process LockApp,LogonUI -ErrorAction SilentlyContinue | Select-Object Name,Id,SessionId | Format-Table -AutoSize | Out-String | Write-Output
Write-Output '=== SUPERVISOR TAIL ==='
Get-Content C:\ProgramData\MahoRD\service.log -Tail 10 | ForEach-Object { Write-Output $_ }
