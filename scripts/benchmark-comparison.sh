#!/usr/bin/env bash
# ix vs ripgrep vs grep — comprehensive comparison benchmark
# Covers: usability, result coverage (recall/precision), speed with/without index
# Usage: ./scripts/benchmark-comparison.sh [--quick] [--json <path>]
set -eo pipefail

IX="${IX_BIN:-target/release/ix}"
RG="${RG_BIN:-rg}"
GREP="${GREP_BIN:-grep}"
PY="${PYTHON3:-python3}"
TIMEOUT="${TIMEOUT_CMD:-timeout}"
BENCH_RUNS=5
QUICK=false
JSON_OUT=""
for arg in "$@"; do
  case "$arg" in --quick) QUICK=true; BENCH_RUNS=3 ;; --json=*) JSON_OUT="${arg#*=}" ;; --help|-h) echo "Usage: $0 [--quick] [--json <path>]"; exit 0 ;; esac
done

TMP=$(mktemp -d /tmp/ix-bench-XXXXXX)
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'

# ── Probe tools ──────────────────────────────────────────────────────────
for tool in "$IX" "$RG" "$GREP" "$PY"; do
  command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: $tool not found (try 'cargo build --release')"; exit 1; }
done

# ── Timing helpers ───────────────────────────────────────────────────────
# wall_ms: runs command, captures stdout+stderr, prints wall-clock ms
wall_ms() {
  local start end
  start=$(date +%s%N)
  "$@" >/dev/null 2>&1 || true
  end=$(date +%s%N)
  echo $(( (end - start) / 1000000 ))
}

# time_n: run benchmark N times, print sorted results (ms)
time_n() {
  local n=$1; shift
  local -a times
  for ((i=0; i<n; i++)); do
    times+=("$(wall_ms "$@")")
  done
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  echo "$sorted"
}

median() {
  local -a arr
  mapfile -t arr <<< "$1"
  local len=${#arr[@]}
  if ((len == 0)); then echo 0; return; fi
  local mid=$((len / 2))
  if ((len % 2 == 0)); then
    echo $(( (arr[mid-1] + arr[mid]) / 2 ))
  else
    echo "${arr[mid]}"
  fi
}

header() { echo -e "\n${CYAN}══ $1 ══${RESET}"; }
subheader() { echo -e "\n${BOLD}$1${RESET}"; }

# ── Corpus generation ────────────────────────────────────────────────────
CORPUS="$TMP/corpus"
generate_corpus() {
  local size=$1  # small=500KB, medium=5MB, large=50MB
  echo "  Generating $size corpus..."
  echo "[DEBUG generate_corpus] TMP=$TMP CORPUS=$CORPUS" >&2
  case "$size" in
    micro)
      mkdir -p "$CORPUS/micro"
      for i in $(seq 1 10); do
        cat > "$CORPUS/micro/f$i.rs" << EOF
fn func_$i() {
    let x = $i;
    println!("hello from func_{}", x);
    let api_key = "sk-$i-a1b2c3d4";
    process(x);
}
EOF
      done
      echo "$CORPUS/micro"
      ;;
    small)
      echo "[DEBUG generate_corpus] Creating small corpus at $CORPUS/small" >&2
      "$PY" -c "
import os, random, string
random.seed(42)
root = '$CORPUS/small'
os.makedirs(root, exist_ok=True)
words = ['fn','let','impl','pub','struct','enum','trait','mod','use','async','await',
 'match','if','else','for','while','loop','return','Result','Option',
 'hello','world','api_key','process','transform','validate','config',
 'handler','middleware','endpoint','database','cache','session','auth']
for i in range(100):
    ext = random.choice(['.rs','.py','.go','.ts','.js','.c'])
    lines = [f'// file {i}']
    for _ in range(random.randint(30,80)):
        w = ' '.join(random.choices(words, k=random.randint(3,8)))
        lines.append(f'{w}; // SAFETY: test pattern')
    with open(os.path.join(root, f'f{i:04d}{ext}'),'w') as f:
        f.write('\n'.join(lines) + '\n')
for term, count in [('authenticate_user', 5), ('IX_SPECIAL_MARKER', 3), ('CACHE_BUSTER_42', 2)]:
    for i in range(count):
        with open(os.path.join(root, f'special_{term}_{i}.rs'),'w') as f:
            f.write(f'fn {term}_{i}() {{ /* {term} in comment */ }}\n')
            f.write(f'let secret = \"{term}_token_{i}\";\n')
"
      echo "[DEBUG generate_corpus] Files created: $(find "$CORPUS/small" -type f 2>/dev/null | wc -l)" >&2
      echo "$CORPUS/small"
      ;;
    small)
      "$PY" -c "
import os, random, string
random.seed(42)
root = '$CORPUS/small'
os.makedirs(root, exist_ok=True)
words = ['fn','let','impl','pub','struct','enum','trait','mod','use','async','await',
         'match','if','else','for','while','loop','return','Result','Option',
         'hello','world','api_key','process','transform','validate','config',
         'handler','middleware','endpoint','database','cache','session','auth']
for i in range(100):
  ext = random.choice(['.rs','.py','.go','.ts','.js','.c'])
  lines = [f'// file {i}']
  for _ in range(random.randint(30,80)):
    w = ' '.join(random.choices(words, k=random.randint(3,8)))
    lines.append(f'{w};  // SAFETY: test pattern')
  with open(os.path.join(root, f'f{i:04d}{ext}'),'w') as f:
    f.write('\n'.join(lines) + '\n')
# Introduce distinctive patterns
for term, count in [('authenticate_user', 5), ('IX_SPECIAL_MARKER', 3), ('CACHE_BUSTER_42', 2)]:
  for i in range(count):
    with open(os.path.join(root, f'special_{term}_{i}.rs'),'w') as f:
      f.write(f'fn {term}_{i}() {{ /* {term} in comment */ }}\n')
      f.write(f'let secret = \"{term}_token_{i}\";\n')
"
      echo "$CORPUS/small"
      ;;
    medium)
      DIR="$CORPUS/medium"
      mkdir -p "$DIR"
      "$PY" -c "
import os, random
random.seed(123)
root = '$DIR'
for i in range(1000):
  ext = random.choice(['.rs','.py','.go','.ts','.js','.c','.java','.kt','.swift'])
  lines = [f'// module {i}']
  for _ in range(random.randint(20,60)):
    lines.append(f'fn check_{random.randint(0,10000)}() -> bool {{ true }}  // SAFETY')
  with open(os.path.join(root, f'{i:05d}{ext}'),'w') as f:
    f.write('\n'.join(lines) + '\n')
# Sprinkle known markers
for i in range(50):
  with open(os.path.join(root, f'auth_{i:03d}.rs'),'w') as f:
    f.write(f'fn verify_token_{i}() {{ /* token verification */ }}\n')
    f.write(f'let token = \"eyJ{i}\";\n')
"
      echo "$DIR"
      ;;
    large)
      DIR="$CORPUS/large"
      mkdir -p "$DIR"
      "$PY" -c "
import os, random
random.seed(456)
root = '$DIR'
for i in range(5000):
  ext = random.choice(['.rs','.py','.go','.ts','.js','.c','.java','.kt','.swift','.cpp','.h'])
  lines = [f'// module {i}']
  for _ in range(random.randint(10,40)):
    lines.append(f'fn compute_{random.randint(0,99999)}() {{ /* routine */ }}')
  with open(os.path.join(root, f'{i:05d}{ext}'),'w') as f:
    f.write('\n'.join(lines) + '\n')
for i in range(200):
  with open(os.path.join(root, f'session_{i:04d}.rs'),'w') as f:
    f.write(f'fn handle_session_{i}() {{ /* session */ }}\n')
    f.write(f'let sid = \"sess_{i}_abc123\";\n')
"
      echo "$DIR"
      ;;
  esac
}

# ── Index builder ────────────────────────────────────────────────────────
build_index() {
  echo "[DEBUG build_index] Building index for $1" >&2
  echo "[DEBUG build_index] Files in dir: $(find "$1" -type f ! -path '*/.ix/*' | wc -l)" >&2
  "$IX" --build "$1" 2>&1
  echo "[DEBUG build_index] exit=$?" >&2
  echo "[DEBUG build_index] .ix size: $(ls -la "$1/.ix/" 2>/dev/null || echo 'none')" >&2
  echo "[DEBUG build_index] shard.ix exists: $(test -f "$1/.ix/shard.ix" && echo yes || echo no)" >&2
  echo "[DEBUG build_index] shard.ix size: $(stat -c%s "$1/.ix/shard.ix" 2>/dev/null || echo 0)" >&2
}

# ── Ground truth: use rg with equivalent flags ──────────────────────────
# Returns sorted unique match lines: file:line:col:content
ground_truth() {
  local dir=$1 pattern=$2 flags=$3
  # rg --no-heading --no-ignore --color=never -n
  local rg_cmd=("$RG" -n --no-heading --no-ignore --color=never)
  IFS=' ' read -ra extra <<< "$flags"
  rg_cmd+=( "${extra[@]}" )
  rg_cmd+=( "$pattern" "$dir" )
  "${rg_cmd[@]}" 2>/dev/null | sort -u || true
}

# ── ix normalized output ────────────────────────────────────────────────
ix_search() {
  local dir=$1 pattern=$2 flags=$3
  local ix_cmd=("$IX")
  IFS=' ' read -ra extra <<< "$flags"
  ix_cmd+=( "${extra[@]}" )
  ix_cmd+=( "$pattern" "$dir" )
  echo "[DEBUG ix_search] CMD: ${ix_cmd[*]}" >&2
  echo "[DEBUG ix_search] dir exists: $(test -d "$dir" && echo yes || echo no)" >&2
  echo "[DEBUG ix_search] .ix exists: $(test -d "$dir/.ix" && echo yes || echo no)" >&2
  local out
  out=$("${ix_cmd[@]}" 2>&1)
  local rc=$?
  echo "[DEBUG ix_search] exit=$rc stderr_len=${#out} stdout_lines=$(echo "$out" | wc -l)" >&2
  if [ $rc -eq 0 ]; then
    echo "$out" | sort -u
  else
    echo "$out" | sort -u
  fi
}

# ── Evaluation: compute recall, precision, F1 ───────────────────────────
# Compare ix output vs rg ground truth
eval_match() {
  local label=$1 pattern=$2 dir=$3 ix_flags=$4 rg_flags=$5
  local ix_out="$TMP/${label}_ix.txt"
  local rg_out="$TMP/${label}_rg.txt"

  echo "[DEBUG eval_match] Before ix_search: pattern='$pattern' dir='$dir' flags='$ix_flags'" >&2
  ix_search "$dir" "$pattern" "$ix_flags" > "$ix_out"
  echo "[DEBUG eval_match] After ix_search: ix_out size=$(stat -c%s "$ix_out" 2>/dev/null || echo 0) lines=$(wc -l < "$ix_out")" >&2
  
  ground_truth "$dir" "$pattern" "$rg_flags" > "$rg_out"

  # Normalize to match keys: file:line (line-based match comparison)
  local ix_keys="$TMP/${label}_ix_keys.txt"
  local rg_keys="$TMP/${label}_rg_keys.txt"
  awk -F: '{print $1":"$2}' "$ix_out" | sort -u > "$ix_keys" 2>/dev/null || true
  awk -F: '{print $1":"$2}' "$rg_out" | sort -u > "$rg_keys" 2>/dev/null || true

  local n_ix n_rg n_common
  n_ix=$(wc -l < "$ix_keys" | tr -d ' ')
  n_rg=$(wc -l < "$rg_keys" | tr -d ' ')
  n_common=$(comm -12 "$ix_keys" "$rg_keys" | wc -l | tr -d ' ')
  echo "[DEBUG eval_match] n_ix=$n_ix n_rg=$n_rg n_common=$n_common" >&2

  local recall precision f1
  local calc_out
  calc_out=$("$PY" -c "
import sys
n_ix = $n_ix; n_rg = $n_rg; n_common = $n_common
recall = 1.0 if n_rg == 0 else round(n_common / n_rg, 4)
precision = 1.0 if n_ix == 0 else round(n_common / n_ix, 4)
if recall + precision == 0:
    f1 = 'N/A'
else:
    f1 = round(2 * (recall * precision) / (recall + precision), 4)
print(f'{recall}|{precision}|{f1}')
" 2>/dev/null || echo "N/A|N/A|N/A")
  IFS='|' read -r recall precision f1 <<< "$calc_out"

  # Write result to file for caller to read
  echo "$label|$n_ix|$n_rg|$n_common|$recall|$precision|$f1" > "$TMP/coverage_result_${label}.txt"
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 1 — USABILITY
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 1: USABILITY"

usability_results=""
usability_tests() {
  local dir="$CORPUS/small"
  subheader "1a: Help and version output"

  # Help text
  if "$IX" --help 2>&1 | grep -qE 'Usage:|Options:|Arguments:'; then
    usability_results+="help_output|PASS|Help text is well-structured\n"
    echo "  PASS help_output"
  else
    usability_results+="help_output|FAIL|Help text missing standard sections\n"
    echo "  FAIL help_output"
  fi

  # Version
  if "$IX" --version 2>&1 | grep -qE '^ix [0-9]'; then
    usability_results+="version_output|PASS|Version reported\n"
    echo "  PASS version_output ($("$IX" --version 2>&1))"
  else
    usability_results+="version_output|FAIL|No version info\n"
    echo "  FAIL version_output"
  fi

  subheader "1b: Error handling"

  # No pattern
  local rc=0
  "$IX" "$dir" 2>/dev/null || rc=$?
  if [ "$rc" -ne 0 ]; then
    usability_results+="no_pattern_error|PASS|Non-zero exit on missing pattern\n"
    echo "  PASS no_pattern_error (exit=$rc)"
  else
    usability_results+="no_pattern_error|WARN|Zero exit with no pattern\n"
    echo "  WARN no_pattern_error"
  fi

  # Invalid regex
  rc=0
  "$IX" --regex '[invalid' "$dir" 2>/dev/null || rc=$?
  if [ "$rc" -ne 0 ]; then
    usability_results+="invalid_regex|PASS|Non-zero exit on invalid regex\n"
    echo "  PASS invalid_regex (exit=$rc)"
  else
    usability_results+="invalid_regex|WARN|Zero exit on invalid regex\n"
    echo "  WARN invalid_regex"
  fi

  # --multiline without --regex
  local err_output
  err_output=$("$IX" -U "hello" "$dir" 2>&1 || true)
  if echo "$err_output" | grep -qiE 'error|requires|--regex'; then
    usability_results+="multiline_requires_regex|PASS|Clear error for -U without --regex\n"
    echo "  PASS multiline_requires_regex"
  else
    usability_results+="multiline_requires_regex|WARN|No clear error for -U without --regex\n"
    echo "  WARN multiline_requires_regex"
  fi

  # Non-existent path
  rc=0
  "$IX" "hello" /nonexistent/path 2>/dev/null || rc=$?
  if [ "$rc" -ne 0 ]; then
    usability_results+="nonexistent_path|PASS|Non-zero exit on bad path\n"
    echo "  PASS nonexistent_path (exit=$rc)"
  else
    usability_results+="nonexistent_path|WARN|Zero exit on bad path\n"
    echo "  WARN nonexistent_path"
  fi

  subheader "1c: Output format consistency"

  # JSON output is valid
  if "$IX" --json "hello" "$dir" 2>/dev/null | "$PY" -c 'import json,sys; [json.loads(l) for l in sys.stdin]' >/dev/null 2>&1; then
    usability_results+="json_valid|PASS|JSON output is valid NDJSON\n"
    echo "  PASS json_valid"
  else
    usability_results+="json_valid|FAIL|JSON output is malformed\n"
    echo "  FAIL json_valid"
  fi

  # --count returns non-negative integer
  local count_out
  count_out=$("$IX" -c "hello" "$dir" 2>/dev/null)
  if echo "$count_out" | grep -qE '^[0-9]+$'; then
    usability_results+="count_output|PASS|Count is integer: $count_out\n"
    echo "  PASS count_output ($count_out)"
  else
    usability_results+="count_output|FAIL|Count not integer: $count_out\n"
    echo "  FAIL count_output"
  fi

  # --files-only doesn't show line numbers
  local files_out
  files_out=$("$IX" -l "fn" "$dir" 2>/dev/null)
  if ! echo "$files_out" | grep -qE ':[0-9]+:'; then
    usability_results+="files_only_no_lines|PASS|No line numbers in -l mode\n"
    echo "  PASS files_only_no_lines"
  else
    usability_results+="files_only_no_lines|WARN|Line numbers found in -l mode\n"
    echo "  WARN files_only_no_lines"
  fi

  # stats output on stderr
  local stats_out
  stats_out=$("$IX" --stats "fn" "$dir" 2>&1)
  if echo "$stats_out" | grep -qE 'trigrams_queried|search_time_ms'; then
    usability_results+="stats_output|PASS|Stats present\n"
    echo "  PASS stats_output"
  else
    usability_results+="stats_output|FAIL|No stats\n"
    echo "  FAIL stats_output"
  fi

  rg_help() { "$RG" --help 2>&1; }
  grep_help() { "$GREP" --help 2>&1; }

  # Compare help verbosity
  local ix_help_len rg_help_len grep_help_len
  ix_help_len=$("$IX" --help 2>&1 | wc -l)
  rg_help_len=$(rg_help | wc -l)
  grep_help_len=$(grep_help | wc -l)
  echo "  Info: help lines — ix=$ix_help_len rg=$rg_help_len grep=$grep_help_len"
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 2 — RESULT COVERAGE (Recall / Precision)
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 2: RESULT COVERAGE (Recall / Precision vs rg)"

echo "  Building index for coverage tests..."
build_index "$CORPUS/small"

coverage_tests() {
  local dir="$CORPUS/small"
  local -a results
  local -a queries=(
    "literal_hello|hello||"
    "literal_fn|fn||"
    "literal_apikey|api_key||"
    "literal_token|eyJ||"
    "literal_nomatch|ZZZ_NOTHING||"
    "regex_fn_body|fn.*\{|--regex"
    "regex_ident|\b[a-z_]+_\d+|--regex"
    "regex_token|token_[0-9a-z]+|--regex"
    "case_sensitive|AUTHENTICATE_USER||"
    "case_insensitive|authenticate_user|-i"
    "word_boundary|hello|-w"
    "unicode_basic|process||"
  )

  echo "  Running ${#queries[@]} coverage queries..."

  for qdef in "${queries[@]}"; do
    IFS='|' read -r label pattern ixflags <<< "$qdef"
    # Determine rg flags equivalent to ix flags
    local rgflags=""
    case "$ixflags" in
      *-i*) rgflags="-i" ;;
      *-w*) rgflags="-w" ;;
      *--regex*) rgflags="-E" ;;
    esac
    local result
    # CALL DIRECTLY instead of $(...)
    eval_match "$label" "$pattern" "$dir" "$ixflags" "$rgflags"
    result=$(cat "$TMP/${label}_ix.txt" | wc -l)
    # We need to re-read the result from the temp file that eval_match creates
    # Actually let's just read the result file
    if [ -f "$TMP/coverage_result_${label}.txt" ]; then
      result=$(cat "$TMP/coverage_result_${label}.txt")
    else
      # fallback
      local ix_keys="$TMP/${label}_ix_keys.txt"
      local rg_keys="$TMP/${label}_rg_keys.txt"
      local n_ix n_rg n_common
      n_ix=$(wc -l < "$ix_keys" | tr -d ' ')
      n_rg=$(wc -l < "$rg_keys" | tr -d ' ')
      n_common=$(comm -12 "$ix_keys" "$rg_keys" | wc -l | tr -d ' ')
      local recall precision f1
      local calc_out
      calc_out=$("$PY" -c "
import sys
n_ix = $n_ix; n_rg = $n_rg; n_common = $n_common
recall = 1.0 if n_rg == 0 else round(n_common / n_rg, 4)
precision = 1.0 if n_ix == 0 else round(n_common / n_ix, 4)
if recall + precision == 0:
    f1 = 'N/A'
else:
    f1 = round(2 * (recall * precision) / (recall + precision), 4)
print(f'{recall}|{precision}|{f1}')
" 2>/dev/null || echo "N/A|N/A|N/A")
      IFS='|' read -r recall precision f1 <<< "$calc_out"
      result="$label|$n_ix|$n_rg|$n_common|$recall|$precision|$f1"
    fi
    results+=("$result")
    IFS='|' read -r lbl n_ix n_rg n_common recall prec f1 <<< "$result"
    printf "  %-25s ix=%4d rg=%4d common=%4d recall=%.2f prec=%.2f f1=%.4s\n" \
      "$lbl" "$n_ix" "$n_rg" "$n_common" "$recall" "$prec" "$f1"
  done

  # Store for report
  for r in "${results[@]}"; do
    echo "$r" >> "$TMP/coverage_results.txt"
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 3 — SPEED: INDEXED (ix with pre-built index vs rg/grep)
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 3: SPEED (Indexed)"

speed_indexed_tests() {
  local -a sizes
  if $QUICK; then sizes=("small"); else sizes=("small" "medium" "large"); fi

  for size in "${sizes[@]}"; do
    local dir="$CORPUS/$size"
    echo "  Building index for $size corpus..."
    build_index "$dir"

    # Query patterns: one that matches nothing, one selective, one broad
    local -a queries=()
    if $QUICK; then
      queries=("hello|hello||" "verif|verify_token_|-r")
    else
      queries=(
        "literal|hello||"
        "selective|verify_token_|-r"
        "broad|fn||"
        "regex_token|token_[0-9a-z]+|--regex"
        "nomatch|ZZZ_EMPTY_12345||"
      )
    fi

    for qdef in "${queries[@]}"; do
      IFS='|' read -r label pattern ixflags <<< "$qdef"
      local runs=$BENCH_RUNS

      # ix indexed (warm)
      local ix_times
      ix_times=$(time_n $runs "$IX" $ixflags "$pattern" "$dir")
      local ix_med
      ix_med=$(median "$ix_times")

      # rg
      local rg_times
      rg_times=$(time_n $runs "$RG" -n --no-heading --no-ignore --color=never $([ "$ixflags" = "-i" ] && echo "-i" || true) $([ "$ixflags" = "--regex" ] && echo "-E" || true) "$pattern" "$dir")
      local rg_med
      rg_med=$(median "$rg_times")

      # grep
      local grep_times
      grep_times=$(time_n $runs "$GREP" -rn --exclude-dir=.ix $([ "$ixflags" = "-i" ] && echo "-i" || true) $([ "$ixflags" = "--regex" ] && echo "-E" || true) "$pattern" "$dir")
      local grep_med
      grep_med=$(median "$grep_times")

      printf "  %-25s corpus=%-6s ix=%'5dms rg=%'5dms grep=%'5dms\n" \
        "$label (indexed)" "$size" "$ix_med" "$rg_med" "$grep_med"
      echo "$size|$label|indexed|$ix_med|$rg_med|$grep_med" >> "$TMP/speed_indexed.txt"
    done
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 4 — SPEED: COLD / NO INDEX (ix --no-index vs rg vs grep)
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 4: SPEED (Cold / No Index)"

speed_cold_tests() {
  local -a sizes
  if $QUICK; then sizes=("small"); else sizes=("small" "medium"); fi

  for size in "${sizes[@]}"; do
    local dir="$CORPUS/$size"

    # Ensure no .ix index present (cold test)
    rm -rf "$dir/.ix"

    local -a queries
    if $QUICK; then
      queries=("hello|hello||")
    else
      queries=("literal|hello||" "selective|verify_token_|-r" "broad|fn||")
    fi

    for qdef in "${queries[@]}"; do
      IFS='|' read -r label pattern ixflags <<< "$qdef"
      local runs=$BENCH_RUNS

      # ix --no-index
      local ix_times
      ix_times=$(time_n $runs "$IX" $ixflags --no-index "$pattern" "$dir")
      local ix_med
      ix_med=$(median "$ix_times")

      # rg
      local rg_times
      rg_times=$(time_n $runs "$RG" -n --no-heading --no-ignore --color=never $([ "$ixflags" = "-i" ] && echo "-i" || true) $([ "$ixflags" = "--regex" ] && echo "-E" || true) "$pattern" "$dir")
      local rg_med
      rg_med=$(median "$rg_times")

      # grep
      local grep_times
      grep_times=$(time_n $runs "$GREP" -rn --exclude-dir=.ix $([ "$ixflags" = "-i" ] && echo "-i" || true) $([ "$ixflags" = "--regex" ] && echo "-E" || true) "$pattern" "$dir")
      local grep_med
      grep_med=$(median "$grep_times")

      printf "  %-25s corpus=%-6s ix=%'5dms rg=%'5dms grep=%'5dms\n" \
        "$label (cold)" "$size" "$ix_med" "$rg_med" "$grep_med"
      echo "$size|$label|cold|$ix_med|$rg_med|$grep_med" >> "$TMP/speed_cold.txt"
    done
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 5 — SCALABILITY (sweep file count, measure crossing point)
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 5: SCALABILITY (ix vs rg at varying corpus sizes)"

scalability_tests() {
  if $QUICK; then
    echo "  Skipping scalability in quick mode"
    return
  fi

  local -a counts=(10 100 1000)
  for nfiles in "${counts[@]}"; do
    local dir="$CORPUS/scalability_$nfiles"
    mkdir -p "$dir"
    rm -f "$dir"/*

    "$PY" -c "
import os, random
random.seed(int('$nfiles'))
root = '$dir'
for i in range($nfiles):
  ext = random.choice(['.rs','.py','.go'])
  lines = [f'// f{i}']
  for _ in range(random.randint(10,30)):
    lines.append(f'fn check_{random.randint(0,9999)}() {{}}')
  with open(os.path.join(root, f'{i:05d}{ext}'),'w') as f:
    f.write('\n'.join(lines) + '\n')
# Add a known pattern
with open(os.path.join(root, 'target.rs'),'w') as f:
    f.write('fn TARGET_FN() {}\nlet TARGET_VAR = 42;\n')
" 2>/dev/null

    # Build index for ix
    build_index "$dir"

    local runs=5
    local pattern="TARGET"

    local ix_times ix_med rg_times rg_med grep_times grep_med
    ix_times=$(time_n $runs "$IX" "$pattern" "$dir")
    ix_med=$(median "$ix_times")
    rg_times=$(time_n $runs "$RG" -n --no-heading --no-ignore --color=never "$pattern" "$dir")
    rg_med=$(median "$rg_times")
    grep_times=$(time_n $runs "$GREP" -rn --exclude-dir=.ix "$pattern" "$dir")
    grep_med=$(median "$grep_times")

    printf "  files=%'5d  ix=%'5dms  rg=%'5dms  grep=%'5dms\n" \
      "$nfiles" "$ix_med" "$rg_med" "$grep_med"
    echo "$nfiles|$ix_med|$rg_med|$grep_med" >> "$TMP/scalability.txt"
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 6 — EDGE CASE PERFORMANCE
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 6: EDGE CASE PERFORMANCE"

edge_perf_tests() {
  local dir="$CORPUS/small"
  build_index "$dir"

  local -a edge_queries=(
    "unicode_utf8|café|"
    "long_pattern|fn.{50,100}return|--regex"
    "many_matches|fn|"
    "case_insensitive_broad|FN|-i"
    "single_char|x|"
    "numeric|\d+|--regex"
  )

  local runs=$BENCH_RUNS
  for qdef in "${edge_queries[@]}"; do
    IFS='|' read -r label pattern ixflags <<< "$qdef"

    local ix_times ix_med rg_times rg_med
    ix_times=$(time_n $runs "$IX" $ixflags "$pattern" "$dir")
    ix_med=$(median "$ix_times")
    rg_times=$(time_n $runs "$RG" -n --no-heading --no-ignore --color=never $([ "$ixflags" = "-i" ] && echo "-i" || true) $([ "$ixflags" = "--regex" ] && echo "-E" || true) "$pattern" "$dir")
    rg_med=$(median "$rg_times")

    printf "  %-30s ix=%'5dms  rg=%'5dms\n" "$label" "$ix_med" "$rg_med"
    echo "$label|$ix_med|$rg_med" >> "$TMP/edge_perf.txt"
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  SECTION 7 — BUILD TIME
# ═══════════════════════════════════════════════════════════════════════════
header "SECTION 7: INDEX BUILD TIME"

build_time_tests() {
  local -a sizes
  if $QUICK; then sizes=("micro"); else sizes=("micro" "small" "medium"); fi

  for size in "${sizes[@]}"; do
    local dir="$CORPUS/$size"
    rm -rf "$dir/.ix"

    local runs=3
    local times
    times=$(time_n $runs "$IX" --build "$dir")
    local med
    med=$(median "$times")

    local nfiles
    nfiles=$(find "$dir" -type f ! -path '*/.ix/*' | wc -l)
    local total_bytes
    total_bytes=$(find "$dir" -type f ! -path '*/.ix/*' -exec stat -c%s {} + 2>/dev/null | awk '{s+=$1} END{print s}')
    echo "  $size: ${nfiles} files, ${total_bytes:-?} bytes — build=${med}ms"
    echo "$size|$nfiles|${total_bytes:-0}|$med" >> "$TMP/build_times.txt"
  done
}

# ═══════════════════════════════════════════════════════════════════════════
#  MAIN
# ═══════════════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}ix Comprehensive Comparison Benchmark${RESET}"
echo "  ix: $("$IX" --version 2>&1)"
echo "  rg: $("$RG" --version 2>&1 | head -1)"
echo "  grep: $("$GREP" --version 2>&1 | head -1)"
echo "  Runs per benchmark: $BENCH_RUNS"
echo "  Quick mode: $QUICK"
echo "  Temp dir: $TMP"

# Generate all corpora upfront
header "CORPUS GENERATION"
echo "  Generating corpora (may take a moment)..."
if $QUICK; then
  generate_corpus micro > /dev/null
  generate_corpus small > /dev/null
else
  generate_corpus micro > /dev/null
  generate_corpus small > /dev/null
  generate_corpus medium > /dev/null
  generate_corpus large > /dev/null
fi

echo "  Micro:   $(find "$CORPUS/micro" -type f 2>/dev/null | wc -l) files"
echo "  Small:   $(find "$CORPUS/small" -type f 2>/dev/null | wc -l) files"
if [ -d "$CORPUS/medium" ]; then echo "  Medium:  $(find "$CORPUS/medium" -type f 2>/dev/null | wc -l) files"; fi
if [ -d "$CORPUS/large" ]; then echo "  Large:   $(find "$CORPUS/large" -type f 2>/dev/null | wc -l) files"; fi

# Run sections
usability_tests
coverage_tests
speed_indexed_tests
speed_cold_tests
scalability_tests
edge_perf_tests
build_time_tests

# ═══════════════════════════════════════════════════════════════════════════
#  REPORT
# ═══════════════════════════════════════════════════════════════════════════
header "COMPLETE REPORT"

report() {
  local report_file="$TMP/report.md"
  exec 5>&1
  exec > "$report_file"

  echo "# ix Comparison Benchmark Report"
  echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "ix: $("$IX" --version 2>&1)"
  echo "rg: $("$RG" --version 2>&1 | head -1)"
  echo "grep: $("$GREP" --version 2>&1 | head -1)"
  echo ""

  # 1. Usability
  echo "## 1. Usability"
  echo ""
  echo "| Test | Result | Detail |"
  echo "|------|--------|--------|"
  local usr="${usability_results//\\n/$'\n'}"
  while IFS='|' read -r test result detail; do
    [ -z "$test" ] && continue
    icon="❌"
    [ "$result" = "PASS" ] && icon="✅"
    [ "$result" = "WARN" ] && icon="⚠️"
    echo "| $test | $icon $result | $detail |"
  done <<< "$usr"
  echo ""

  # 2. Coverage
  echo "## 2. Result Coverage (vs rg)"
  echo ""
  echo "| Query | ix matches | rg matches | Common | Recall | Precision | F1 |"
  echo "|-------|-----------|-----------|--------|--------|-----------|----|"
  if [ -f "$TMP/coverage_results.txt" ]; then
    while IFS='|' read -r lbl n_ix n_rg n_common recall prec f1; do
      echo "| $lbl | $n_ix | $n_rg | $n_common | $recall | $prec | $f1 |"
    done < "$TMP/coverage_results.txt"
  fi
  echo ""

  # 3. Speed indexed
  echo "## 3. Speed (Indexed)"
  echo ""
  echo "| Corpus | Query | Mode | ix (ms) | rg (ms) | grep (ms) | Winner |"
  echo "|--------|-------|------|---------|---------|-----------|--------|"
  if [ -f "$TMP/speed_indexed.txt" ]; then
    while IFS='|' read -r size label mode ix_ms rg_ms grep_ms; do
      local winner="rg"
      if [ "$ix_ms" -le "$rg_ms" ] && [ "$ix_ms" -le "$grep_ms" ]; then winner="ix"; fi
      if [ "$rg_ms" -le "$ix_ms" ] && [ "$rg_ms" -le "$grep_ms" ]; then winner="rg"; fi
      if [ "$grep_ms" -le "$ix_ms" ] && [ "$grep_ms" -le "$rg_ms" ]; then winner="grep"; fi
      echo "| $size | $label | $mode | $ix_ms | $rg_ms | $grep_ms | $winner |"
    done < "$TMP/speed_indexed.txt"
  fi
  echo ""

  # 4. Speed cold
  echo "## 4. Speed (Cold / No Index)"
  echo ""
  echo "| Corpus | Query | Mode | ix (ms) | rg (ms) | grep (ms) | Winner |"
  echo "|--------|-------|------|---------|---------|-----------|--------|"
  if [ -f "$TMP/speed_cold.txt" ]; then
    while IFS='|' read -r size label mode ix_ms rg_ms grep_ms; do
      local winner="rg"
      if [ "$ix_ms" -le "$rg_ms" ] && [ "$ix_ms" -le "$grep_ms" ]; then winner="ix"; fi
      if [ "$rg_ms" -le "$ix_ms" ] && [ "$rg_ms" -le "$grep_ms" ]; then winner="rg"; fi
      if [ "$grep_ms" -le "$ix_ms" ] && [ "$grep_ms" -le "$rg_ms" ]; then winner="grep"; fi
      echo "| $size | $label | $mode | $ix_ms | $rg_ms | $grep_ms | $winner |"
    done < "$TMP/speed_cold.txt"
  fi
  echo ""

  # 5. Scalability
  echo "## 5. Scalability (file count sweep)"
  echo ""
  echo "| Files | ix (ms) | rg (ms) | grep (ms) | Winner |"
  echo "|-------|---------|---------|-----------|--------|"
  if [ -f "$TMP/scalability.txt" ]; then
    while IFS='|' read -r nfiles ix_ms rg_ms grep_ms; do
      local winner="rg"
      if [ "$ix_ms" -le "$rg_ms" ] && [ "$ix_ms" -le "$grep_ms" ]; then winner="ix"; fi
      if [ "$rg_ms" -le "$ix_ms" ] && [ "$rg_ms" -le "$grep_ms" ]; then winner="rg"; fi
      if [ "$grep_ms" -le "$ix_ms" ] && [ "$grep_ms" -le "$rg_ms" ]; then winner="grep"; fi
      echo "| $nfiles | $ix_ms | $rg_ms | $grep_ms | $winner |"
    done < "$TMP/scalability.txt"
  fi
  echo ""

  # 6. Edge perf
  echo "## 6. Edge Case Performance"
  echo ""
  echo "| Query | ix (ms) | rg (ms) | Winner |"
  echo "|-------|---------|---------|--------|"
  if [ -f "$TMP/edge_perf.txt" ]; then
    while IFS='|' read -r label ix_ms rg_ms; do
      local winner="rg"
      [ "$ix_ms" -le "$rg_ms" ] && winner="ix"
      echo "| $label | $ix_ms | $rg_ms | $winner |"
    done < "$TMP/edge_perf.txt"
  fi
  echo ""

  # 7. Build time
  echo "## 7. Index Build Time"
  echo ""
  echo "| Corpus | Files | Size (bytes) | Build Time (ms) |"
  echo "|--------|-------|-------------|-----------------|"
  if [ -f "$TMP/build_times.txt" ]; then
    while IFS='|' read -r size nfiles bytes build_ms; do
      echo "| $size | $nfiles | $bytes | $build_ms |"
    done < "$TMP/build_times.txt"
  fi
  echo ""

  exec 1>&5 5>&-
  cat "$report_file"

  # JSON output if requested
  if [ -n "$JSON_OUT" ]; then
    echo "  Writing JSON to $JSON_OUT"
    "$PY" -c "
import json, os

results = {}

# Coverage
results['coverage'] = []
cov_file = '$TMP/coverage_results.txt'
if os.path.exists(cov_file):
    with open(cov_file) as f:
        for line in f:
            parts = line.strip().split('|')
            if len(parts) >= 7:
                results['coverage'].append({
                    'query': parts[0],
                    'ix_matches': int(parts[1]),
                    'rg_matches': int(parts[2]),
                    'common': int(parts[3]),
                    'recall': float(parts[4]),
                    'precision': float(parts[5]),
                    'f1': float(parts[6]) if parts[6] != 'N/A' else None
                })

# Speed indexed
results['speed_indexed'] = []
f = '$TMP/speed_indexed.txt'
if os.path.exists(f):
    with open(f) as fh:
        for line in fh:
            parts = line.strip().split('|')
            if len(parts) >= 6:
                results['speed_indexed'].append({
                    'corpus': parts[0], 'query': parts[1], 'mode': parts[2],
                    'ix_ms': int(parts[3]), 'rg_ms': int(parts[4]),
                    'grep_ms': int(parts[5])
                })

# Speed cold
results['speed_cold'] = []
f = '$TMP/speed_cold.txt'
if os.path.exists(f):
    with open(f) as fh:
        for line in fh:
            parts = line.strip().split('|')
            if len(parts) >= 6:
                results['speed_cold'].append({
                    'corpus': parts[0], 'query': parts[1], 'mode': parts[2],
                    'ix_ms': int(parts[3]), 'rg_ms': int(parts[4]),
                    'grep_ms': int(parts[5])
                })

# Scalability
results['scalability'] = []
f = '$TMP/scalability.txt'
if os.path.exists(f):
    with open(f) as fh:
        for line in fh:
            parts = line.strip().split('|')
            if len(parts) >= 4:
                results['scalability'].append({
                    'files': int(parts[0]),
                    'ix_ms': int(parts[1]), 'rg_ms': int(parts[2]),
                    'grep_ms': int(parts[3])
                })

# Edge perf
results['edge_perf'] = []
f = '$TMP/edge_perf.txt'
if os.path.exists(f):
    with open(f) as fh:
        for line in fh:
            parts = line.strip().split('|')
            if len(parts) >= 3:
                results['edge_perf'].append({
                    'query': parts[0], 'ix_ms': int(parts[1]), 'rg_ms': int(parts[2])
                })

# Build times
results['build_times'] = []
f = '$TMP/build_times.txt'
if os.path.exists(f):
    with open(f) as fh:
        for line in fh:
            parts = line.strip().split('|')
            if len(parts) >= 4:
                results['build_times'].append({
                    'corpus': parts[0], 'files': int(parts[1]),
                    'bytes': int(parts[2]), 'build_ms': int(parts[3])
                })

with open('$JSON_OUT', 'w') as jf:
    json.dump(results, jf, indent=2)
print('  JSON report written')
"
  fi
}

report 2>&1

echo ""
echo -e "${GREEN}Benchmark complete.${RESET} Report saved to variables."
