$ErrorActionPreference = "Continue"
$k = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
Set-ItemProperty $k -Name PromptOnSecureDesktop -Value 1
Set-ItemProperty $k -Name ConsentPromptBehaviorAdmin -Value 2
Remove-Item C:\erd\uac2.log -ErrorAction SilentlyContinue
Set-Content -Path C:\erd\uac3-inner.ps1 -Value '$log = "C:\erd\uac2.log"; "START" | Out-File $log -Encoding ASCII; try { Start-Process -FilePath "C:\Windows\System32\cmd.exe" -ArgumentList "/c ping -n 300 127.0.0.1" -Verb RunAs; "issued" | Out-File $log -Append -Encoding ASCII } catch { $_.Exception.Message | Out-File $log -Append -Encoding ASCII }' -Encoding ASCII
Set-Content -Path C:\erd\run-uac3.bat -Value "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\erd\uac3-inner.ps1" -Encoding ASCII
schtasks /delete /tn maho-uac9 /f 2>&1 | Out-Null
$when = (Get-Date).AddMinutes(2).ToString("HH:mm")
schtasks /create /tn maho-uac9 /tr C:\erd\run-uac3.bat /sc once /st $when /it /f | Out-Null
schtasks /run /tn maho-uac9 | Out-Null
foreach ($n in 1..10) {
  Start-Sleep -Seconds 2
  $c = Get-Process consent -ErrorAction SilentlyContinue
  $hint = (Get-Content C:\ProgramData\MahoRD\input-desktop.txt -Raw).Trim()
  if ($c -ne $null) { Write-Output ("P" + $n + " CONSENT=" + $c.Id + " HINT=" + $hint); break } else { Write-Output ("P" + $n + " none HINT=" + $hint) }
}
