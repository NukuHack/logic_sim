#!/usr/bin/env bash
# Unified check/build script for logic_sim.
# Usage: ./build.sh [-y | -n | -q]
#   -y  Run all checks and tests, then build
#   -n  Skip everything except build
#   -q  Quick: only unit tests (cargo test --lib), then build
set -euo pipefail

MAX_LINE_WIDTH=150
MAX_FILE_LINES=500

RUN_ALL=false
SKIP_ALL=false
QUICK=false

# Parse flags
while [[ $# -gt 0 ]]; do
  case "$1" in
    -y) RUN_ALL=true; shift ;;
    -n) SKIP_ALL=true; shift ;;
    -q) QUICK=true; shift ;;
    *) echo "Usage: $0 [-y | -n | -q]"; echo "  -y  Run all checks and tests, then build"; echo "  -n  Skip everything except build"; echo "  -q  Quick: only unit tests, then build"; exit 1 ;;
  esac
done

if ! $RUN_ALL && ! $SKIP_ALL && ! $QUICK; then
  echo "Usage: $0 [-y | -n | -q]"
  echo "  -y  Run all checks and tests, then build"
  echo "  -n  Skip everything except build"
  echo "  -q  Quick: only unit tests, then build"
  exit 1
fi

# ── Build prerequisites (always needed) ──
echo "==> Checking prerequisites..."
command -v rustup >/dev/null || { echo "ERROR: rustup not found. Install from https://rustup.rs"; exit 1; }
# wasm-pack is no longer required for native builds; uncomment if you later add WASM support
# command -v wasm-pack >/dev/null || { echo "INFO: Installing wasm-pack..."; cargo install wasm-pack; }

# ── Checks & Tests ──
if $SKIP_ALL; then
  echo "==> Skipping all checks and tests (-n flag)."
else
  # Formatting and linting (skip for quick mode)
  if ! $QUICK; then
    echo "==> Running cargo fmt"
    cargo fmt

    echo "==> Running cargo clippy"
    cargo clippy -- -D warnings

    echo "==> Checking file and line length (warnings only)"
    find . -type f -name "*.rs" -not -path "./target/*" | while IFS= read -r file; do
      total_lines=$(wc -l < "$file")
      if [ "$total_lines" -gt "$MAX_FILE_LINES" ]; then
        echo "WARN: $file has $total_lines lines (limit: $MAX_FILE_LINES) — consider splitting into submodules"
      fi
      awk -v file="$file" -v max="$MAX_LINE_WIDTH" '
        {
          if (length($0) > max) {
            printf "WARN: %s:%d exceeds %d chars (%d)\n", file, NR, max, length($0)
          }
        }
      ' "$file"
    done
  fi

  # Quick mode: unit tests only. Full mode (-y) covers them in the complete
  # suite below, so don't run them twice.
  if ! $RUN_ALL; then
    echo "==> Running pure-logic tests (cargo test --lib)..."
    set +e
    cargo test --lib
    LIB_TEST_STATUS=$?
    set -e
    if [ "$LIB_TEST_STATUS" -ne 0 ]; then
      echo "==> WARNING: pure-logic tests failed (exit $LIB_TEST_STATUS). Continuing anyway."
    fi
  fi

  # Full test suite + Miri (only in -y, best-effort)
  if $RUN_ALL; then
    echo "==> Running all tests (unit, integration, docs)..."
    set +e
    cargo test
    ALL_TEST_STATUS=$?
    set -e
    if [ "$ALL_TEST_STATUS" -ne 0 ]; then
      echo "==> WARNING: full test suite failed (exit $ALL_TEST_STATUS). Continuing anyway."
    fi
    if command -v cargo-miri >/dev/null 2>&1 || cargo +nightly miri --version >/dev/null 2>&1; then
      echo "==> Running cargo miri test --lib"
      # -Zmiri-disable-isolation: the unit tests round-trip real save files
      # through std::fs (statx & co.), which Miri only permits with host
      # access. Without this every fs-backed test dies on an unsupported-
      # operation error instead of actually checking our code for UB.
      if ! MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test --lib; then
        echo "WARN: cargo miri test --lib failed or found UB — investigate before trusting unsafe code"
      fi
    else
      echo "==> Skipping Miri (nightly + 'miri' rustup component not found)"
    fi
  fi
fi

# ── Build ──
echo "==> Building release..."
cargo build --release

echo "==> Build complete. Binary available at target/release/app."
