#!/bin/zsh
# usage: all.sh OUTDIR : compares every round with the current release binary
S=${NDB_ROUNDS:?set NDB_ROUNDS to a work folder}
OUT=$1
BIN=${2:-$S/target/release/examples/model_select}
mkdir -p $OUT
for r in f1 f2 f3 f4 f5 f6 f7 f8 f9 f10 f11 f12 f13 f14 f15 f16 f17 f18 f19 f20 f21 f22 f23 f24 f25 f26 f27 f28 f29 f30 f31 f32 f33 f34 f35 f36 f37 f38 f39 f40 f41 f42 f43 f44 f45 f46 r2 r3 r4 r5; do
  [ -d $S/$r/results ] || { print -- "--- $r: no results"; continue; }
  MODEL_SELECT=$BIN timeout 170 python3 $S/hard/cmp_list.py $S/$r/decks $S/$r/results --show 8 > $OUT/$r.txt 2>&1
  print -- "--- $r: $(grep -E '^(agree|both-stop|DIFFER|HSPICE-STOPS-ONLY|NDB-ERROR\+DIFF|NDB-ERROR-ONLY|NDB-CRASH|no-lis|alter-unmeasured|temper-unmeasured|empty-lis) ' $OUT/$r.txt | sed -E 's/ +/ /' | tr '\n' ';')"
done
