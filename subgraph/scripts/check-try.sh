#!/bin/sh
# 04-subgraph.md R4: every contract call in a mapping goes through a try_
# variant, so a revert never reaches the handler. Counts the direct
# non-try_ method calls on bound contracts; the count must be zero.
set -eu
cd "$(dirname "$0")/.."
# A bound contract call looks like `<name>.try_<fn>(` or `<name>.<fn>(` where
# <name> was assigned from `<Abi>.bind(`. Collect the variable names first.
names=$(grep -ho '[A-Za-z_][A-Za-z0-9_]* = [A-Za-z]*\.bind(' src/*.ts | sed 's/ = .*//' | sort -u)
bad=0
for n in $names; do
  # direct calls on the bound variable that are not try_ calls
  hits=$(grep -n "\b$n\.[a-zA-Z_][a-zA-Z0-9_]*(" src/*.ts | grep -v "\b$n\.try_" || true)
  if [ -n "$hits" ]; then
    echo "non-try_ call on bound contract '$n':"
    echo "$hits"
    bad=1
  fi
done
# Inline `.bind(addr).try_x()` chains are fine; inline non-try_ chains are not.
inline=$(grep -n '\.bind([^)]*)\.[a-zA-Z_]' src/*.ts | grep -v '\.bind([^)]*)\.try_' || true)
if [ -n "$inline" ]; then
  echo "inline non-try_ call:"
  echo "$inline"
  bad=1
fi
binds=$(grep -o '\.bind(' src/*.ts | wc -l | tr -d ' ')
tries=$(grep -o '\.try_[a-zA-Z_]*(' src/*.ts | wc -l | tr -d ' ')
echo "bind() sites: $binds, try_ calls: $tries, non-try_ calls: $bad"
exit $bad
