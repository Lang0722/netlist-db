#!/bin/zsh
# usage: vm.sh push NAME DECKDIR | run NAME [PARALLEL DEADLINE CAP] | back NAME OUTDIR
S=${NDB_ROUNDS:?set NDB_ROUNDS to a work folder}
CL=$(git -C "${0:A:h}" rev-parse --show-toplevel)
X=$CL/tmp/xfer
W='\\Mac\Home\Project\netlist-db-hspice-test\tmp\xfer'
case $1 in
  push) tar czf $X/$2.tar.gz -C $3 decks && ls -la $X/$2.tar.gz | cut -c20-80 ;;
  run)  P=${3:-4}; D=${4:-150}; C=${5:-4.5}; T=$((D+25))
        timeout $T prlctl exec "Windows 11" powershell -NoProfile -ExecutionPolicy Bypass -File "$W\\vmrun.ps1" -Name $2 -Parallel $P -DeadlineSec $D -CapGB $C $6 2>&1 | tr -d '\r' ;;
  back) timeout 170 prlctl exec "Windows 11" powershell -NoProfile -ExecutionPolicy Bypass -File "$W\\vmback.ps1" -Name $2 2>&1 | tr -d '\r' | tail -2
        mkdir -p $3; tar xzf $X/$2.out.tar.gz -C $3; rm -f $X/$2.out.tar.gz $X/$2.tar.gz
        # the licence lines are not stored
        for f in $3/results/*.lis(N); do grep -a -v -E '^ *lic:' $f > $f.tmp && mv $f.tmp $f; done
        for f in $3/results/*.stdout(N); do grep -a -v -E '^ *lic:' $f > $f.tmp && mv $f.tmp $f; done
        grep -a -l -E '^ *lic:' $3/results/*(N) | wc -l | tr -d ' ' | sed 's/^/files with licence lines left: /' ;;
esac
