#!/usr/bin/env bash
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ERRORS=0
grep -q "cargo test --lib" "docs/README.md" || { echo "ERROR: README test command missing"; ERRORS=$((ERRORS+1)); }
grep -q "pub fn execute" "src/lib/api.rs" || { echo "ERROR: process_request missing"; ERRORS=$((ERRORS+1)); }
[ -f "lcov.info" ] || { echo "ERROR: lcov.info missing"; ERRORS=$((ERRORS+1)); }
for f in fuzz/fuzz_targets/fuzz_parse_trigram.rs fuzz/fuzz_targets/fuzz_parse_posting.rs; do
  [ -f "$f" ] || { echo "ERROR: $f missing"; ERRORS=$((ERRORS+1)); }
done
if [ $ERRORS -eq 0 ]; then
  echo "All docs verified."
  exit 0
else
  echo "Found $ERRORS error(s)"
  exit 1
fi
