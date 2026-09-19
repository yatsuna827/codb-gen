#!/usr/bin/env bash
# gen の生成速度計測(ベンチ)と、部分実行出力のsha256回帰チェックを行うスクリプト。
# target-cpu=native は .cargo/config.toml があれば cargo build --release 時に自動で効く。
# 全周期(limit=2^32)の完全なバイト一致確認は別途行う。このスクリプトは
# 高速な部分実行による速度計測と回帰検出を目的としている。
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release

BIN="./target/release/codb-gen.exe"
LIMIT=0x1000000
THREADS=4

# 現在のコードで一度 threads=4 実行して得た実値(出力はスレッド数非依存)。
EXPECTED_SHA="3787778a6e616e9cbd89abbd311b3e5dce10ceb1b2b8a206b744c2a386185533"

# --- 回帰: 1回実行してsha256を期待値と比較 ---
OUT="$(mktemp)"
"$BIN" gen --out "$OUT" --limit "$LIMIT" --threads "$THREADS"
ACTUAL_SHA="$(sha256sum "$OUT" | cut -d' ' -f1)"
rm -f "$OUT"

if [ "$ACTUAL_SHA" = "$EXPECTED_SHA" ]; then
    echo "REGRESSION: PASS (sha256=$ACTUAL_SHA)"
else
    echo "REGRESSION: FAIL"
    echo "  expected: $EXPECTED_SHA"
    echo "  actual:   $ACTUAL_SHA"
    exit 1
fi

# --- ベンチ: 同条件で計3回実行し、phase1+2(orbit scan)所要時間の最小値・中央値を出す ---
declare -a TIMES=()
for i in 1 2 3; do
    BOUT="$(mktemp)"
    LOG="$(mktemp)"
    "$BIN" gen --out "$BOUT" --limit "$LIMIT" --threads "$THREADS" >"$LOG" 2>&1
    T="$(grep -oE 'phase1\+2 \(orbit scan\) done in [0-9.]+s' "$LOG" | grep -oE 'done in [0-9.]+s' | sed -E 's/done in ([0-9.]+)s/\1/')"
    TIMES+=("$T")
    rm -f "$BOUT" "$LOG"
done

# 昇順ソートして最小値・中央値(3件中の真ん中)を求める
SORTED=($(printf '%s\n' "${TIMES[@]}" | sort -n))
MIN="${SORTED[0]}"
MEDIAN="${SORTED[1]}"

echo "BENCH: min=${MIN}s median=${MEDIAN}s (limit=$LIMIT, threads=$THREADS, 3 runs)"
