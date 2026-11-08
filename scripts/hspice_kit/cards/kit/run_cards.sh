#!/bin/bash
# Runs HSPICE on each deck of decks/, from inside decks/ (so that the `.lib`
# file next to the decks is found), writes results/, and packs them into
# hspice_card_results.tar.gz.
#   ./run_cards.sh            (HSPICE=/path/to/hspice ./run_cards.sh)
cd "$(dirname "$0")"
HSPICE=${HSPICE:-hspice}
mkdir -p results
"$HSPICE" -v > results/hspice_version.txt 2>&1 || true
cd decks
for deck in *.sp; do
  name=$(basename "$deck" .sp)
  echo "== $name"
  "$HSPICE" -i "$deck" -o "../results/$name" > "../results/$name.stdout" 2>&1 \
    || echo "   (hspice exited with status $?: see results/$name.stdout and results/$name.lis)"
done
cd ..
uname -a > results/machine.txt 2>/dev/null || true
tar czf hspice_card_results.tar.gz results
echo "done: hspice_card_results.tar.gz"
