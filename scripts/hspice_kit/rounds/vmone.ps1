# Runs one deck in a folder below the kit's decks folder, with cwd = decks. Touches C:\tmp\ndbkit only.
param([string]$Name, [string]$Deck, [string]$Out)
$hspice = 'C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe'
$decks = "C:\tmp\ndbkit\$Name\decks"
Set-Location $decks
Copy-Item "$Deck`_" $Deck -Force
$p = Start-Process -FilePath $hspice -ArgumentList "-i $Deck -o ..\results\$Out" -WorkingDirectory $decks -PassThru -NoNewWindow -RedirectStandardOutput "..\results\$Out.stdout" -RedirectStandardError "..\results\$Out.stderr"
$null = $p.Handle
$null = $p.WaitForExit(60000)
if (-not $p.HasExited) { & taskkill /F /T /PID $p.Id | Out-Null; Write-Output 'KILLED' }
Select-String -Path "..\results\$Out.lis" -Pattern 'element name|l drawn|w drawn|\*\*error|job concluded|job aborted' | ForEach-Object { $_.Line }
