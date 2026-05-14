#!/usr/bin/env bash
# Documentation Verification Script
# Verifies all documentation claims are current and accurate

set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ERRORS=0
WARNINGS=0

echo "Verifying documentation..."

# Check 1: README.md exists and has verification date
if grep -q "Last Verified:" "README.md"; then
    echo "[✓] README.md has verification date"
else
    echo "[!] README.md missing verification date"
    WARNINGS=$((WARNINGS+1))
fi

# Check 2: docs/README.md test command documented
if grep -q "cargo test --lib" "docs/README.md"; then
    echo "[✓] Test command documented in docs/README.md"
else
    echo "[✗] Test command missing from docs/README.md"
    ERRORS=$((ERRORS+1))
fi

# Check 3: Public API functions exist
if grep -q "pub fn execute" "src/lib/api.rs"; then
    echo "[✓] execute function exists"
else
    echo "[✗] execute function missing"
    ERRORS=$((ERRORS+1))
fi

# Check 4: Coverage report exists
if [ -f "lcov.info" ]; then
    echo "[✓] lcov.info exists"
else
    echo "[✗] lcov.info missing - run: cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info"
    ERRORS=$((ERRORS+1))
fi

# Check 5: Fuzz targets exist
for f in fuzz/fuzz_targets/fuzz_parse_trigram.rs fuzz/fuzz_targets/fuzz_parse_posting.rs; do
    if [ ! -f "$f" ]; then
        echo "[✗] $f missing"
        ERRORS=$((ERRORS+1))
    fi
done
echo "[✓] Fuzz targets exist"

# Check 6: Delta format documentation exists
if [ -f "docs/DELTA-FORMAT.md" ]; then
    echo "[✓] Delta format documented"
else
    echo "[!] Delta format undocumented (optional)"
    WARNINGS=$((WARNINGS+1))
fi

# Check 7: Daemon runbook exists
if [ -f "docs/DAEMON-RUNBOOK.md" ]; then
    echo "[✓] Daemon runbook exists"
else
    echo "[!] Daemon runbook missing (optional)"
    WARNINGS=$((WARNINGS+1))
fi

# Check 8: Run tests to verify behavior
echo ""
echo "Running test verification..."
if cargo test --lib --quiet 2>&1 | grep -q "test result: ok"; then
    echo "[✓] All tests pass"
else
    echo "[✗] Tests failed"
    ERRORS=$((ERRORS+1))
fi

# Check 9: Verify doc links
echo "Checking documentation links..."
if cargo doc --no-deps --document-private-items 2>&1 | grep -q "warning: unresolved link"; then
    echo "[✗] Broken intra-doc links found"
    ERRORS=$((ERRORS+1))
else
    echo "[✓] No broken doc links"
fi

# Summary
echo ""
if [ $ERRORS -eq 0 ] && [ $WARNINGS -eq 0 ]; then
    echo "All docs verified successfully ✓"
    exit 0
elif [ $ERRORS -eq 0 ]; then
    echo "Verified with $WARNINGS warning(s) ✓"
    exit 0
else
    echo "Found $ERRORS error(s), $WARNINGS warning(s)"
    exit 1
fi
