$ErrorActionPreference = "Continue"
Write-Output "=== ACL on the machine-wide MahoRD dir ==="
(Get-Acl C:\ProgramData\MahoRD).Access | ForEach-Object { Write-Output ($_.IdentityReference.ToString() + " | " + $_.FileSystemRights + " | " + $_.AccessControlType) }
Write-Output "=== ACL on workers dir (created by the service) ==="
(Get-Acl C:\ProgramData\MahoRD\workers).Access | ForEach-Object { Write-Output ($_.IdentityReference.ToString() + " | " + $_.FileSystemRights + " | " + $_.AccessControlType) }
Write-Output "=== ACL on the pairing store (credentials!) ==="
if (Test-Path C:\ProgramData\MahoRD\host-authorizations.json) { (Get-Acl C:\ProgramData\MahoRD\host-authorizations.json).Access | ForEach-Object { Write-Output ($_.IdentityReference.ToString() + " | " + $_.FileSystemRights + " | " + $_.AccessControlType) } } else { Write-Output "NO_STORE" }
