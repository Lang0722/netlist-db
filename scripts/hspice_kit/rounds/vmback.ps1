# Packs the results of a run, without the licence lines, for the host.
param([string]$Name, [int]$MaxMB = 2000)
$ErrorActionPreference = 'Stop'
$share = '\\Mac\Home\Project\netlist-db-hspice-test\tmp\xfer'
$root = "C:\tmp\ndbkit\$Name"
Set-Location $root
if (Test-Path "$Name.out.tar.gz") { Remove-Item "$Name.out.tar.gz" }
Get-ChildItem results -Filter *.ic0 -ErrorAction SilentlyContinue | Remove-Item
Get-ChildItem results -Filter *.st0 -ErrorAction SilentlyContinue | Remove-Item
Get-ChildItem results -Filter *.pa0 -ErrorAction SilentlyContinue | Remove-Item
& C:\WINDOWS\system32\tar.exe czf "$Name.out.tar.gz" results
Copy-Item "$Name.out.tar.gz" "$share\$Name.out.tar.gz"
Write-Output ("packed MB: " + [int]((Get-Item "$Name.out.tar.gz").Length / 1MB))
