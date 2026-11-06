# Copies a deck folder to the VM, runs HSPICE on each deck with a deadline and a
# memory cap, and returns the results. Touches C:\tmp\ndbkit only.
param([string]$Name, [int]$Parallel = 4, [int]$DeadlineSec = 150, [double]$CapGB = 4.5, [switch]$Resume)
$ErrorActionPreference = 'Stop'
$hspice = 'C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe'
$share = '\\Mac\Home\Project\netlist-db-hspice-test\tmp\xfer'
$root = "C:\tmp\ndbkit\$Name"
if (-not $Resume) {
  if (Test-Path $root) { Remove-Item -Recurse -Force $root }
  New-Item -ItemType Directory -Force "$root\results" | Out-Null
  Copy-Item "$share\$Name.tar.gz" "$root\in.tar.gz"
  Set-Location $root
  & C:\WINDOWS\system32\tar.exe xzf in.tar.gz
  Remove-Item in.tar.gz
}
Set-Location $root
$decks = Join-Path $root 'decks'
function IsDone($n) {
  $l = "$root\results\$n.lis"
  (Test-Path $l) -and (Select-String -Path $l -Pattern 'job concluded|job aborted' -Quiet)
}
# hspice.exe starts a child process that does the work: count the whole tree
function TreeBytes($id) {
  $sum = 0.0
  $self = Get-CimInstance Win32_Process -Filter "ProcessId=$id"
  if ($self) { $sum += [double]$self.PrivatePageCount }
  foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$id")) { $sum += TreeBytes $c.ProcessId }
  $sum
}
$todo = New-Object System.Collections.Queue
Get-ChildItem $decks -Filter *.sp | Sort-Object Name | ForEach-Object {
  if (-not (IsDone $_.BaseName)) { $todo.Enqueue($_.BaseName) }
}
Write-Output ("to run: " + $todo.Count)
$running = @{}
$peak = @{}
$sw = [Diagnostics.Stopwatch]::StartNew()
$killed = 0
while (($todo.Count -gt 0 -or $running.Count -gt 0) -and $sw.Elapsed.TotalSeconds -lt $DeadlineSec) {
  foreach ($k in @($running.Keys)) {
    $p = $running[$k]
    if ($p.HasExited) {
      $running.Remove($k)
    } else {
      try {
        $p.Refresh()
        $mem = (TreeBytes $p.Id) / 1GB
        if ($mem -gt $peak[$k]) { $peak[$k] = $mem }
        if ($mem -gt $CapGB) {
          & taskkill /F /T /PID $p.Id | Out-Null
          Write-Output ("KILLED at memory cap {0:n2} GB after {1:n0}s: {2}" -f $mem, $sw.Elapsed.TotalSeconds, $k)
          $killed++
        }
      } catch {}
    }
  }
  while ($running.Count -lt $Parallel -and $todo.Count -gt 0) {
    $n = $todo.Dequeue()
    $p = Start-Process -FilePath $hspice -ArgumentList "-i $n.sp -o ..\results\$n" -WorkingDirectory $decks -PassThru -NoNewWindow -RedirectStandardOutput "$root\results\$n.stdout" -RedirectStandardError "$root\results\$n.stderr"
    $null = $p.Handle
    $running[$n] = $p
    $peak[$n] = 0.0
  }
  Start-Sleep -Milliseconds 200
}
foreach ($k in @($running.Keys)) {
  if (-not $running[$k].HasExited) { & taskkill /F /T /PID $running[$k].Id | Out-Null; Write-Output "KILLED at deadline: $k"; $killed++ }
}
Write-Output ("left in queue: " + $todo.Count + "; killed: " + $killed + "; elapsed " + [int]$sw.Elapsed.TotalSeconds + "s")
$big = $peak.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 3
foreach ($b in $big) { Write-Output ("peak {0}: {1:n3} GB" -f $b.Key, $b.Value) }
