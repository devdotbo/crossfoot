#!/usr/bin/env bash
# The Crossfoot demo, end to end, from the checked-in fixtures and without
# the network. Runs from a clean clone with cargo present:
#
#   bash scripts/demo.sh
#
# Every step writes under one temporary directory (printed at the end and
# kept, so the bundles and pages can be opened) and the script exits
# non-zero on the first failure.
#
# Steps:
#   1. svZCHF demo window, replayed from the fixture bundle's raw responses
#      into a fresh bundle: verdict, summary, root hash.
#   2. Midas customFeed family, replayed from its fixture archive the same
#      way: survey line, verdict, root hash.
#   3. render: static pages over the bundles just written.
#   4. consume --replay: the consumer agent over the recorded subgraph
#      responses, ALLOW or REVIEW per feed.
#   5. verify: every bundle written above, re-hashed and recomputed.
#   6. bundle pack: the svZCHF bundle as one deterministic archive, then
#      verify on the archive alone.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

say() { printf '\n== %s\n' "$*"; }
line() { grep -E "$1" "$2" | head -n "${3:-1}"; }

# One summary line out of a result.json without needing jq: the headline
# and the consumer action of the summary block.
headline() {
  grep -o '"headline": "[^"]*"' "$1" | head -n 1 | sed 's/"headline": //'
}
consumer_action() {
  grep -o '"consumer_action": "[^"]*"' "$1" | head -n 1 | sed 's/"consumer_action": //'
}

say "Building crossfoot (release)"
cargo build --release -p crossfoot --quiet
crossfoot="$root/target/release/crossfoot"
"$crossfoot" --version

work="$(mktemp -d "${TMPDIR:-/tmp}/crossfoot-demo.XXXXXX")"
mkdir -p "$work/bundles"
bundles=()

svzchf_fixture="cli/tests/fixtures/svzchf-demo-24570000-25853000"

say "1. svZCHF, the exact control: demo window 24570000 to 25853000, replayed from the fixture"
"$crossfoot" run svzchf --window demo --from-bundle "$svzchf_fixture" --verify-root "$work" \
  | tee "$work/svzchf.out"
svzchf_bundle="$(sed -n 's/^bundle  *//p' "$work/svzchf.out")"
bundles+=("$svzchf_bundle")
printf 'summary block:  headline %s, consumer_action %s\n' \
  "$(headline "$svzchf_bundle/result.json")" "$(consumer_action "$svzchf_bundle/result.json")"
printf 'fixture result.json sha256 equals the replay:  '
if cmp -s "$svzchf_fixture/result.json" "$svzchf_bundle/result.json"; then
  echo "yes"
else
  echo "NO"
  exit 1
fi

say "2. Midas customFeed family: every round attributed and replayed against the bound in force"
midas_archive="cli/tests/fixtures/midas-25884405.tar.gz"
if [ ! -f "$midas_archive" ]; then
  echo "skipped: no fixture at $midas_archive (spec 02 R19)."
else
  mkdir -p "$work/fixtures"
  tar -xzf "$midas_archive" -C "$work/fixtures"
  midas_fixture=""
  for candidate in "$work"/fixtures/midas-*/; do
    if [ -d "$candidate" ]; then
      midas_fixture="${candidate%/}"
      break
    fi
  done
  [ -n "$midas_fixture" ] || { echo "the archive holds no midas-* directory"; exit 1; }
  # The pinned block of the fixture, from its manifest summary (entries carry
  # hex block strings, the summary the number).
  midas_block="$(grep -o '"block": [0-9][0-9]*' "$midas_fixture/manifest.json" | head -n 1 | tr -dc '0-9')"
  "$crossfoot" run midas --block "$midas_block" --feeds config/midas-mainnet.json \
    --from-bundle "$midas_fixture" --verify-root "$work" \
    | tee "$work/midas.out" | grep -E "^(nav_recomputation|survey|verdict|bundle|root hash|network calls) |mRE7\.customFeed |mTBILL\.customFeed "
  midas_bundle="$(sed -n 's/^bundle  *//p' "$work/midas.out")"
  bundles+=("$midas_bundle")
  printf 'summary block:  headline %s, consumer_action %s\n' \
    "$(headline "$midas_bundle/result.json")" "$(consumer_action "$midas_bundle/result.json")"
  printf 'fixture result.json equals the replay:  '
  if cmp -s "$midas_fixture/result.json" "$midas_bundle/result.json"; then
    echo "yes"
  else
    echo "NO"
    exit 1
  fi
fi

say "3. render: static pages over the bundles, no script, no fetch at view time"
"$crossfoot" render --bundles "$work/bundles" --out "$work/site" | head -n 4

say "4. consume --replay: ALLOW or REVIEW per feed from the recorded subgraph responses"
consume_fixture="cli/tests/fixtures/consume-fixture-v1"
"$crossfoot" consume \
  --replay "$consume_fixture" \
  --feeds "$consume_fixture/feeds.json" \
  --midas-config "$consume_fixture/midas-mainnet.json" \
  --queries subgraph/queries \
  --out "$work/decisions" \
  --now 1788289368 \
  | tee "$work/consume.out" | grep -iE "svzchf|mRE7\.|^decisions|^head|^decided" | head -n 12

say "5. verify: every bundle written above, re-hashed and recomputed without the network"
for bundle in "${bundles[@]}"; do
  echo "-- $bundle"
  "$crossfoot" verify "$bundle" | grep -E "^(target|entries|root hash|replay|status) "
done

say "6. pack and verify: the bundle as one downloadable archive, verified from the archive alone"
"$crossfoot" bundle pack "$svzchf_bundle" --out "$work/$(basename "$svzchf_bundle").tar.gz" \
  | tee "$work/pack.out"
archive="$(sed -n 's/^archive  *//p' "$work/pack.out")"
"$crossfoot" verify "$archive" | grep -E "^(archive|archive sha256|root hash|replay|status) "

say "Done. Everything is under $work"
echo "   bundles:   $work/bundles"
echo "   site:      $work/site/index.html"
echo "   decisions: $work/decisions"
