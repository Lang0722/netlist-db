#!/usr/bin/env bash
# Offline tests of run_hspice.sh. A stub `prlctl` on PATH stands in for the
# VM, so no HSPICE run is needed.
#
# Usage: golden/tools/test_run_hspice.sh
#
# Stub behavior (environment variables):
# - run call (`prlctl exec <vm> powershell ...`): prints STUB_RUN_OUT, exits
#   with STUB_RUN_RC (default 0).
# - fetch call (`prlctl exec <vm> cmd /c type ...`): prints the file
#   STUB_LIS.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/bin"
cat > "$work/bin/prlctl" <<'EOF'
#!/usr/bin/env bash
case "${3-}" in
  powershell) printf '%s' "${STUB_RUN_OUT-}"; exit "${STUB_RUN_RC:-0}" ;;
  cmd) cat "${STUB_LIS:?}"; exit 0 ;;
esac
echo "stub prlctl: unexpected call: $*" >&2
exit 99
EOF
chmod +x "$work/bin/prlctl"
export PATH="$work/bin:$PATH"
export NDBGOLD_FETCH_SLEEP=0

fails=0
ntests=0

# listing <start time text> [ending]: a small listing with CRLF line ends
# and licence lines.
listing() {
  printf ' ****** HSPICE -- P-2019.06-SP1-1 win64 (Sep 27 2019) ******\r\n'
  printf '  Start time: %s\r\n' "$1"
  printf '  lic: HOSTID: 0123456789ab         PID:      1\r\n'
  printf '  lic: 27000@HOST\r\n'
  printf '          ***** %s\r\n' "${2:-job concluded}"
}

# expect <name> <want: ok|fail> <run rc> <run stdout> <listing file>
# Runs the script on a fresh case dir that holds an old hspice.lis. On "ok",
# the case's hspice.lis must be the stub listing without CR and without its
# licence lines (`lic:`). On "fail", the
# exit status must be non-zero and the old hspice.lis must be unchanged.
expect() {
  local name=$1 want=$2 rc=$3 out=$4 lis=$5
  local dir="$work/case_$ntests"
  ntests=$((ntests + 1))
  mkdir -p "$dir"
  printf '* deck\nr1 1 0 1\n.end\n' > "$dir/deck.sp"
  printf 'OLD LISTING\n' > "$dir/hspice.lis"
  STUB_RUN_RC=$rc STUB_RUN_OUT=$out STUB_LIS=$lis \
    "$here/run_hspice.sh" "$dir" > "$work/stdout" 2> "$work/stderr"
  local got=$?
  if [[ $want == ok ]]; then
    if (( got != 0 )); then
      echo "FAIL: $name: exit $got, want 0; stderr:"; sed 's/^/    /' "$work/stderr"
      fails=$((fails + 1))
    elif ! diff -q <(tr -d '\r' < "$lis" | sed '/^[[:space:]]*lic:/d') "$dir/hspice.lis" > /dev/null; then
      echo "FAIL: $name: stored hspice.lis is not the fetched listing"
      fails=$((fails + 1))
    elif grep -q 'lic:' "$dir/hspice.lis"; then
      echo "FAIL: $name: stored hspice.lis has a licence line"
      fails=$((fails + 1))
    fi
  else
    if (( got == 0 )); then
      echo "FAIL: $name: exit 0, want non-zero"
      fails=$((fails + 1))
    elif [[ $(cat "$dir/hspice.lis") != "OLD LISTING" ]]; then
      echo "FAIL: $name: the old hspice.lis was overwritten"
      fails=$((fails + 1))
    fi
  fi
}

start=$'NDBGOLD_START 20261031224429\r\nNDBGOLD_DONE\r\n'
listing "Sat Oct 31 22:44:30 2026" > "$work/fresh.lis"
listing "Sat Oct 31 22:44:29 2026" > "$work/same_second.lis"
listing "Sat Oct 31 21:58:14 2026" > "$work/stale.lis"
listing "Sat Oct 31 22:44:30 2026" "job aborted" > "$work/aborted.lis"
listing "Sat Oct 31 22:44:30 2026" "something else" > "$work/incomplete.lis"
printf '          ***** job concluded\r\n' > "$work/no_start.lis"
listing "Tue Dec  1 00:00:01 2026" > "$work/next_month.lis"

expect "fresh listing" ok 0 "$start" "$work/fresh.lis"
expect "listing started in the run's second" ok 0 "$start" "$work/same_second.lis"
expect "aborted job is a result" ok 0 "$start" "$work/aborted.lis"
expect "month and day padding" ok 0 $'NDBGOLD_START 20261130235959\r\nNDBGOLD_DONE\r\n' "$work/next_month.lis"
expect "run call fails" fail 1 "$start" "$work/fresh.lis"
expect "run call times out" fail 124 "" "$work/fresh.lis"
expect "run call stops before HSPICE" fail 0 $'NDBGOLD_START 20261031224429\r\n' "$work/fresh.lis"
expect "listing older than the run" fail 0 "$start" "$work/stale.lis"
expect "listing without a start time" fail 0 "$start" "$work/no_start.lis"
expect "incomplete listing" fail 0 "$start" "$work/incomplete.lis"
expect "nothing fetched" fail 0 "$start" "$work/does_not_exist.lis"

echo "test_run_hspice: $ntests tests, $fails failure(s)"
(( fails == 0 ))
