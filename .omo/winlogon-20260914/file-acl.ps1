$ErrorActionPreference = "Continue"
$f = "C:\ProgramData\MahoRD\host-authorizations.json"
$acl = Get-Acl $f
Write-Output ("protected=" + $acl.AreAccessRulesProtected)
$acl.Access | ForEach-Object { Write-Output ($_.IdentityReference.ToString() + " | " + $_.FileSystemRights + " | inherited=" + $_.IsInherited) }
