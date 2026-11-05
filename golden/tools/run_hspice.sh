#!/usr/bin/env bash
# Run one golden case through HSPICE on the Parallels VM and store hspice.lis.
#
# Usage: golden/tools/run_hspice.sh <case_dir>
#
# - Every regular file of <case_dir> except hspice.lis and expected.json is
#   copied (base64) into C:\tmp\ndbgold\<case>\ on the VM, and
#   `hspice.exe -i deck.sp -o hspice` runs there. All of this is ONE
#   `prlctl exec "Windows 11" powershell -NoProfile -Command ...` call.
# - The listing is fetched with `prlctl exec ... cmd /c type ...` and written
#   to <case_dir>/hspice.lis with CR stripped (LF line endings) and with no
#   licence lines (`lic:`: host name, host ID, licence server).
# - Access is through prlctl only (never ssh). Every call is bounded by
#   `timeout 180`.
#
# Checks (on failure the script exits non-zero and leaves <case_dir>/hspice.lis
# as it was):
# - The run call must succeed: prlctl exits 0 and the PowerShell command gets
#   to its end (marker NDBGOLD_DONE). A failed upload, a missing hspice.exe or
#   a timeout fails the run (exit 3). A deck that HSPICE aborts is a result,
#   not a failure.
# - The fetched listing must be complete ("job concluded" or "job aborted")
#   and must not be older than the run: its "Start time:" must not be before
#   the VM time that the run call printed just before HSPICE started (marker
#   NDBGOLD_START, same VM clock). Else exit 1.
#
# NDBGOLD_FETCH_SLEEP (seconds, default 3) is the wait between fetch attempts.
set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "usage: $0 <case_dir>" >&2
  exit 2
fi

case_dir=$(cd "$1" && pwd)
case_name=$(basename "$case_dir")
vm="Windows 11"
hspice='C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe'
remote="C:\\tmp\\ndbgold\\${case_name}"

if [[ ! -f "$case_dir/deck.sp" ]]; then
  echo "error: $case_dir/deck.sp is missing" >&2
  exit 2
fi
if [[ ! "$case_name" =~ ^[A-Za-z0-9_]+$ ]]; then
  echo "error: case name must be [A-Za-z0-9_]+: $case_name" >&2
  exit 2
fi

# Build the PowerShell command. It contains no '$' and no loops: those broke
# the quoting through prlctl before. `trap` makes any error (failed upload,
# missing hspice.exe) end the command with exit code 3 before NDBGOLD_DONE.
ps="trap { exit 3 };"
ps+=" Remove-Item -Recurse -Force '${remote}' -ErrorAction SilentlyContinue;"
ps+=" New-Item -ItemType Directory -Force '${remote}' -ErrorAction Stop | Out-Null;"
nfiles=0
while IFS= read -r -d '' f; do
  name=$(basename "$f")
  case "$name" in
    hspice.lis | expected.json | .* ) continue ;;
  esac
  if [[ ! "$name" =~ ^[A-Za-z0-9_.-]+$ ]]; then
    echo "error: unsupported file name: $name" >&2
    exit 2
  fi
  b64=$(base64 < "$f" | tr -d '\n')
  ps+=" [IO.File]::WriteAllBytes('${remote}\\${name}',[Convert]::FromBase64String('${b64}'));"
  nfiles=$((nfiles + 1))
done < <(find "$case_dir" -maxdepth 1 -type f -print0 | sort -z)

ps+=" Set-Location -ErrorAction Stop '${remote}';"
ps+=" Write-Output ('NDBGOLD_START ' + (Get-Date -Format yyyyMMddHHmmss));"
ps+=" & '${hspice}' -i deck.sp -o hspice | Out-Null;"
ps+=" Write-Output NDBGOLD_DONE"

if (( ${#ps} > 30000 )); then
  echo "error: case files too large for one command line (${#ps} chars)" >&2
  exit 2
fi

echo "[$case_name] upload $nfiles file(s) and run HSPICE" >&2
rc=0
run_out=$(timeout 180 prlctl exec "$vm" powershell -NoProfile -Command "$ps" 2>&1 | tr -d '\r') || rc=$?
run_start=$(sed -n 's/^NDBGOLD_START \([0-9]\{14\}\)$/\1/p' <<< "$run_out" | head -n 1)
if (( rc != 0 )) || ! grep -qx NDBGOLD_DONE <<< "$run_out" || [[ -z $run_start ]]; then
  echo "[$case_name] error: the run call failed (exit $rc); hspice.lis is not changed" >&2
  tail -n 5 <<< "$run_out" | sed 's/^/    /' >&2
  exit 3
fi

# listing_start <file>: the listing's "Start time: Sat Oct 31 21:58:14 2026"
# as yyyymmddHHMMSS, or nothing.
listing_start() {
  awk '/^ *Start time:/ {
    mon = index("JanFebMarAprMayJunJulAugSepOctNovDec", $4)
    t = $6; gsub(/:/, "", t)
    if (mon > 0 && t ~ /^[0-9]{6}$/) printf "%04d%02d%02d%s\n", $7, (mon + 2) / 3, $5, t
    exit
  }' "$1"
}

# Fetch the listing. hspice.exe is a wrapper around hspice.com; retry a few
# times in case the listing is not complete yet.
out="$case_dir/hspice.lis"
tmp="$out.tmp"
trap 'rm -f "$tmp" "$tmp.clean"' EXIT
ok=0
for attempt in 1 2 3 4 5; do
  : > "$tmp"
  timeout 180 prlctl exec "$vm" cmd /c "type ${remote}\\hspice.lis" 2>/dev/null | tr -d '\r' > "$tmp" || true
  lis_start=$(listing_start "$tmp")
  if ! grep -Eq 'job concluded|job aborted' "$tmp"; then
    why="listing not complete"
  elif [[ -z $lis_start ]]; then
    why="listing has no 'Start time:'"
  elif (( 10#$lis_start < 10#$run_start )); then
    why="listing is older than the run (start $lis_start < run $run_start)"
  else
    ok=1
    break
  fi
  echo "[$case_name] $why (attempt $attempt)" >&2
  sleep "${NDBGOLD_FETCH_SLEEP:-3}"
done
if (( ! ok )); then
  echo "[$case_name] error: $why; hspice.lis is not changed" >&2
  exit 1
fi
# the licence lines name the host and the licence server: not stored
sed '/^[[:space:]]*lic:/d' "$tmp" > "$tmp.clean"
mv "$tmp.clean" "$out"

if grep -q 'job concluded' "$out"; then
  echo "[$case_name] job concluded" >&2
else
  echo "[$case_name] job ABORTED (see $out)" >&2
fi
