$ErrorActionPreference = "Stop"
$src = Join-Path $env:APPDATA "MahoRD"
$dst = Join-Path $env:ProgramData "MahoRD"
New-Item -ItemType Directory -Force -Path $dst | Out-Null
foreach ($name in @("host-authorizations.json","pairing-keys.json","client-pairings.json")) {
  $from = Join-Path $src $name
  if (Test-Path $from) { Copy-Item $from (Join-Path $dst $name) -Force; Write-Output "COPIED $name" }
  else { Write-Output "ABSENT $name" }
}
Write-Output "STORE_DIR=$dst"
Get-ChildItem $dst -Name | ForEach-Object { Write-Output "PRESENT $_" }
