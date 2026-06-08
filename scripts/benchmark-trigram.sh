#!/usr/bin/env bash
# ix vs rg vs csearch vs zoekt vs ugrep-index — Comprehensive trigram benchmark
# Competitor set per ChatGPT discussion: rg (scan), csearch (canonical), zoekt (modern), ugrep-index (grep-like)
set -eo pipefail

TOOLS="/workspace/ix/.bench-tools"
export PATH="$TOOLS/bin:$PATH"
IX="${IX_BIN:-ix}"
CORPUS="${CORPUS:-/workspace/training_data}"
BENCH_RUNS=5

TMP=$(mktemp -d /workspace/ix-bench-trigram-XXXXXX)
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
LATENCY_FILE="$TMP/latency.txt"
CORRECTNESS_FILE="$TMP/correctness.txt"

echo "Probing tools..."
for tool in "$IX" rg cindex csearch zoekt-index zoekt ugrep ugrep-indexer python3; do
  command -v "$tool" >/dev/null 2>&1 && echo "  FOUND: $tool" || echo "  MISSING: $tool"
done

wall_ms() { local s e; s=$(date +%s%N); "$@" >/dev/null 2>&1 || true; e=$(date +%s%N); echo $(( (e - s) / 1000000 )); }

time_n() {
  local n=$1; shift
  local -a t
  for ((i=0;i<n;i++)); do t+=("$(wall_ms "$@")"); done
  printf '%s\n' "${t[@]}" | sort -n
}

pctl() { local -a a; mapfile -t a <<< "$1"; local l=${#a[@]}; ((l==0))&&echo 0&&return; local i=$(( (l*$2+99)/100-1 )); ((i<0))&&i=0; ((i>=l))&&i=$((l-1)); echo "${a[i]}"; }
median() { pctl "$1" 50; }
p95() { pctl "$1" 95; }

header() { echo -e "\n${CYAN}══ $1 ══${RESET}"; }

# Corpus stats
header "CORPUS"
CORPUS_BYTES=$(find "$CORPUS" -type f -exec stat -c%s {} + 2>/dev/null | awk '{s+=$1} END{print s}')
CORPUS_GB=$(python3 -c "print(round($CORPUS_BYTES / (1024**3), 2))" 2>/dev/null || echo "?")
FILE_COUNT=$(find "$CORPUS" -type f | wc -l)
echo "Path: $CORPUS"
echo "Size: ${CORPUS_GB}GB"
echo "Files: $FILE_COUNT"

# ── Index Building ───────────────────────────────────────
header "INDEX BUILD"

IX_BUILD_MS=0 IX_SIZE=0
echo "  ix..."
rm -rf "$CORPUS/.ix"
t0=$(date +%s%N); "$IX" --build "$CORPUS" 2>/dev/null; t1=$(date +%s%N)
IX_BUILD_MS=$(( (t1 - t0) / 1000000 ))
[ -f "$CORPUS/.ix/shard.ix" ] && IX_SIZE=$(stat -c%s "$CORPUS/.ix/shard.ix")
echo "    build: ${IX_BUILD_MS}ms  size: $(numfmt --to=iec $IX_SIZE 2>/dev/null || echo $IX_SIZE)"

CSEARCH_BUILD_MS=0 CSEARCH_SIZE=0
export CSEARCHINDEX="$TMP/csearch.idx"
echo "  csearch..."
rm -f "$CSEARCHINDEX"
t0=$(date +%s%N); cindex "$CORPUS" 2>/dev/null; t1=$(date +%s%N)
CSEARCH_BUILD_MS=$(( (t1 - t0) / 1000000 ))
[ -f "$CSEARCHINDEX" ] && CSEARCH_SIZE=$(stat -c%s "$CSEARCHINDEX")
echo "    build: ${CSEARCH_BUILD_MS}ms  size: $(numfmt --to=iec $CSEARCH_SIZE 2>/dev/null || echo $CSEARCH_SIZE)"

ZOEKT_BUILD_MS=0 ZOEKT_SIZE=0
ZOEKT_DIR="/workspace/.bench-zoekt"
echo "  zoekt..."
rm -rf "$ZOEKT_DIR"; mkdir -p "$ZOEKT_DIR"
t0=$(date +%s%N); zoekt-index -index "$ZOEKT_DIR" "$CORPUS" 2>/dev/null; t1=$(date +%s%N)
ZOEKT_BUILD_MS=$(( (t1 - t0) / 1000000 ))
[ -d "$ZOEKT_DIR" ] && ZOEKT_SIZE=$(du -sb "$ZOEKT_DIR" 2>/dev/null | cut -f1)
echo "    build: ${ZOEKT_BUILD_MS}ms  size: $(numfmt --to=iec $ZOEKT_SIZE 2>/dev/null || echo $ZOEKT_SIZE)"

UGREP_BUILD_MS=0 UGREP_SIZE=0
echo "  ugrep..."
t0=$(date +%s%N); ugrep-indexer "$CORPUS" 2>/dev/null; t1=$(date +%s%N)
UGREP_BUILD_MS=$(( (t1 - t0) / 1000000 ))
UGREP_SIZE=$(find "$CORPUS" -name "_UG#*" -type d 2>/dev/null | xargs -r du -sb 2>/dev/null | awk '{s+=$1} END{print s+0}')
[ -z "$UGREP_SIZE" ] && UGREP_SIZE=0
echo "    build: ${UGREP_BUILD_MS}ms  size: $(numfmt --to=iec $UGREP_SIZE 2>/dev/null || echo $UGREP_SIZE)"

# ── Index Economics ──────────────────────────────────────
header "INDEX STORAGE ECONOMICS"
printf "%-10s %8s %7s %10s %10s %8s\n" "Tool" "CorpusGB" "Files" "Build(ms)" "Size" "Idx/Corp"
printf "%-10s %8s %7d %10s %10s %8s\n" "ix" "$CORPUS_GB" "$FILE_COUNT" "$IX_BUILD_MS" "$(numfmt --to=iec $IX_SIZE 2>/dev/null || echo $IX_SIZE)" "$(python3 -c "print(round($IX_SIZE/$CORPUS_BYTES,4))" 2>/dev/null)"
printf "%-10s %8s %7d %10s %10s %8s\n" "csearch" "$CORPUS_GB" "$FILE_COUNT" "$CSEARCH_BUILD_MS" "$(numfmt --to=iec $CSEARCH_SIZE 2>/dev/null || echo $CSEARCH_SIZE)" "$(python3 -c "print(round($CSEARCH_SIZE/$CORPUS_BYTES,4))" 2>/dev/null)"
printf "%-10s %8s %7d %10s %10s %8s\n" "zoekt" "$CORPUS_GB" "$FILE_COUNT" "$ZOEKT_BUILD_MS" "$(numfmt --to=iec $ZOEKT_SIZE 2>/dev/null || echo $ZOEKT_SIZE)" "$(python3 -c "print(round($ZOEKT_SIZE/$CORPUS_BYTES,4))" 2>/dev/null)"
printf "%-10s %8s %7d %10s %10s %8s\n" "ugrep" "$CORPUS_GB" "$FILE_COUNT" "$UGREP_BUILD_MS" "$(numfmt --to=iec $UGREP_SIZE 2>/dev/null || echo $UGREP_SIZE)" "$(python3 -c "print(round($UGREP_SIZE/$CORPUS_BYTES,4))" 2>/dev/null)"

echo ""
echo "Bytes indexed per second:"
printf "  ix:      %.0f MB/s\n"  $(python3 -c "print($CORPUS_BYTES/($IX_BUILD_MS/1000)/(1024**2))" 2>/dev/null)
printf "  csearch: %.0f MB/s\n"  $(python3 -c "print($CORPUS_BYTES/($CSEARCH_BUILD_MS/1000)/(1024**2))" 2>/dev/null)
printf "  zoekt:   %.0f MB/s\n"  $(python3 -c "print($CORPUS_BYTES/($ZOEKT_BUILD_MS/1000)/(1024**2))" 2>/dev/null)
printf "  ugrep:   %.0f MB/s\n"  $(python3 -c "print($CORPUS_BYTES/(max(1,$UGREP_BUILD_MS)/1000)/(1024**2))" 2>/dev/null || echo "N/A")

# ── Query Definitions ────────────────────────────────────
declare -A QLABEL QPATTERN QEXTRA QCLASS
QLABEL[0]="unique_uuid";       QPATTERN[0]="d4e5f6a7-b8c9-0123-4567-890123456789"; QEXTRA[0]="";       QCLASS[0]="rare literal"
QLABEL[1]="common_model";      QPATTERN[1]="gpt-4";                                   QEXTRA[1]="";       QCLASS[1]="medium literal"
QLABEL[2]="tool_marker";       QPATTERN[2]="function_call";                           QEXTRA[2]="";       QCLASS[2]="agent-log literal"
QLABEL[3]="error_common";      QPATTERN[3]="error";                                   QEXTRA[3]="";       QCLASS[3]="common literal"
QLABEL[4]="no_match";          QPATTERN[4]="ZXY_NOMATCH_99999_XYZ";                   QEXTRA[4]="";       QCLASS[4]="no-match"
QLABEL[5]="secret_pattern";    QPATTERN[5]="sk-[A-Za-z0-9_-]+";                      QEXTRA[5]="regex";  QCLASS[5]="secret regex"
QLABEL[6]="fn_pattern";        QPATTERN[6]="fn [a-z_]+";                              QEXTRA[6]="regex";  QCLASS[6]="simple regex"
QLABEL[7]="error_class";       QPATTERN[7]="[A-Z][A-Za-z0-9]+Error";                 QEXTRA[7]="regex";  QCLASS[7]="symbolic regex"
QLABEL[8]="alternation";       QPATTERN[8]="TODO|FIXME|HACK";                        QEXTRA[8]="regex";  QCLASS[8]="alternation"
QLABEL[9]="case_insensitive";  QPATTERN[9]="sk-";                                     QEXTRA[9]="-i";     QCLASS[9]="case-insensitive"
QLABEL[10]="file_filtered";    QPATTERN[10]="def ";                                   QEXTRA[10]="filter"; QCLASS[10]="filtered .py"
QLABEL[11]="long_phrase";      QPATTERN[11]="model.*temperature.*max_tokens";         QEXTRA[11]="regex"; QCLASS[11]="phrase regex"

# ── Latency Benchmark ────────────────────────────────────
run_query() {
  local tool=$1 label=$2 pattern=$3 extra=$4 class=$5
  local runs=$BENCH_RUNS
  local -a cmd

  case $tool in
    ix)
      cmd=("$IX")
      [ "$extra" = "regex" ] && cmd+=(--regex)
      [ "$extra" = "-i" ] && cmd+=(-i)
      cmd+=("$pattern" "$CORPUS")
      ;;
    rg)
      cmd=(rg -n --no-heading --no-ignore --color=never)
      [ "$extra" = "-i" ] && cmd+=(-i)
      cmd+=("$pattern" "$CORPUS")
      ;;
    csearch)
      export CSEARCHINDEX="$TMP/csearch.idx"
      cmd=(csearch)
      [ "$extra" = "-i" ] && cmd+=(-i)
      cmd+=("$pattern")
      ;;
    zoekt)
      cmd=(zoekt -index "$ZOEKT_DIR")
      cmd+=("$pattern")
      ;;
    ugrep)
      cmd=(ugrep --index -n)
      [ "$extra" = "-i" ] && cmd+=(-i)
      if [ "$extra" = "regex" ]; then cmd+=(-E); fi
      if echo "$pattern" | grep -qE '\[|\||\*|\+|\{'; then cmd+=(-E); fi
      cmd+=("$pattern" "$CORPUS")
      ;;
  esac

  local times
  times=$(time_n $runs "${cmd[@]}")
  local m=$(median "$times")
  local p95v=$(p95 "$times")
  local count=$("${cmd[@]}" 2>/dev/null | wc -l) || true

  echo "$tool|$label|$m|$p95v|$count" >> "$LATENCY_FILE"
  printf "  %-8s %-20s p50=%5dms p95=%5dms matches=%d\n" "$tool" "$label" "$m" "$p95v" "$count"
}

header "QUERY LATENCY"
for tool in ix rg csearch zoekt ugrep; do
  for i in $(seq 0 11); do
    run_query "$tool" "${QLABEL[$i]}" "${QPATTERN[$i]}" "${QEXTRA[$i]}" "${QCLASS[$i]}"
  done
done

# ── Search Correctness ───────────────────────────────────
header "SEARCH CORRECTNESS VS RG"
printf "%-22s %8s %8s %8s %8s %8s\n" "Query" "ix" "rg" "csearch" "zoekt" "ugrep"
for i in $(seq 0 11); do
  label="${QLABEL[$i]}"
  pattern="${QPATTERN[$i]}"
  extra="${QEXTRA[$i]}"

  # ix
  ix_c=0
  ix_cmd=("$IX")
  [ "$extra" = "regex" ] && ix_cmd+=(--regex)
  [ "$extra" = "-i" ] && ix_cmd+=(-i)
  ix_cmd+=("$pattern" "$CORPUS")
  ix_c=$("${ix_cmd[@]}" 2>/dev/null | wc -l) || true

  # rg
  rg_c=0
  rg_cmd=(rg -n --no-heading --no-ignore --color=never)
  [ "$extra" = "-i" ] && rg_cmd+=(-i)
  rg_cmd+=("$pattern" "$CORPUS")
  rg_c=$("${rg_cmd[@]}" 2>/dev/null | wc -l) || true

  # csearch
  export CSEARCHINDEX="$TMP/csearch.idx"
  cs_c=$(csearch "$pattern" 2>/dev/null | wc -l) || true

  # zoekt
  zo_c=$(zoekt -index "$ZOEKT_DIR" "$pattern" 2>/dev/null | wc -l) || true

  # ugrep
  ug_c=0
  ug_cmd=(ugrep --index -n)
  [ "$extra" = "-i" ] && ug_cmd+=(-i)
  [ "$extra" = "regex" ] && ug_cmd+=(-E)
  if echo "$pattern" | grep -qE '\[|\||\*|\+|\{'; then ug_cmd+=(-E); fi
  ug_cmd+=("$pattern" "$CORPUS")
  ug_c=$("${ug_cmd[@]}" 2>/dev/null | wc -l) || true

  printf "%-22s %8d %8d %8d %8d %8d\n" "$label" "$ix_c" "$rg_c" "$cs_c" "$zo_c" "$ug_c"
  echo "$label|$ix_c|$rg_c|$cs_c|$zo_c|$ug_c" >> "$CORRECTNESS_FILE"
done

# ── Report ───────────────────────────────────────────────
header "COMPLETE REPORT"
R="$TMP/report.md"
{
  echo "# ix Trigram Benchmark Report"
  echo ""
  echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "ix: $("$IX" --version 2>&1)"
  echo "rg: $(rg --version 2>&1 | head -1)"
  echo "csearch: $(csearch -h 2>&1 | head -1)"
  echo "zoekt: $(zoekt -help 2>&1 | head -1)"
  echo "ugrep: $(ugrep --version 2>&1 | head -1)"  
  echo ""

  echo "## Corpus"
  echo "- Path: $CORPUS"
  echo "- Size: ${CORPUS_GB}GB (${CORPUS_BYTES} bytes)"
  echo "- Files: $FILE_COUNT"
  echo "- Type: Real agent training data (code, logs, sessions, config)"
  echo ""

  echo "## Index Storage Economics"
  echo ""
  echo "| Tool | Corpus GB | Files | Build time | Index size | Index/corpus |"
  echo "|------|-----------|-------|------------|------------|--------------|"
  echo "| ix | $CORPUS_GB | $FILE_COUNT | ${IX_BUILD_MS}ms | $(numfmt --to=iec $IX_SIZE 2>/dev/null) | $(python3 -c "print(round($IX_SIZE/$CORPUS_BYTES,4))" 2>/dev/null) |"
  echo "| csearch | $CORPUS_GB | $FILE_COUNT | ${CSEARCH_BUILD_MS}ms | $(numfmt --to=iec $CSEARCH_SIZE 2>/dev/null) | $(python3 -c "print(round($CSEARCH_SIZE/$CORPUS_BYTES,4))" 2>/dev/null) |"
  echo "| zoekt | $CORPUS_GB | $FILE_COUNT | ${ZOEKT_BUILD_MS}ms | $(numfmt --to=iec $ZOEKT_SIZE 2>/dev/null) | $(python3 -c "print(round($ZOEKT_SIZE/$CORPUS_BYTES,4))" 2>/dev/null) |"
  echo "| ugrep | $CORPUS_GB | $FILE_COUNT | ${UGREP_BUILD_MS}ms | $(numfmt --to=iec $UGREP_SIZE 2>/dev/null) | $(python3 -c "print(round($UGREP_SIZE/$CORPUS_BYTES,4))" 2>/dev/null) |"
  echo ""
  echo "Break-even queries vs rg:"
  if [ -f "$LATENCY_FILE" ]; then
    IX_AVG=$(awk -F'|' '$1=="ix" {s+=$3; n++} END{print int(s/n)}' "$LATENCY_FILE")
    RG_AVG=$(awk -F'|' '$1=="rg" {s+=$3; n++} END{print int(s/n)}' "$LATENCY_FILE")
    if [ "${IX_AVG:-0}" -gt 0 ] && [ "${RG_AVG:-0}" -gt "$IX_AVG" ]; then
      RG_SAVED=$((RG_AVG - IX_AVG))
      BREAK_EVEN=$(( (IX_BUILD_MS / RG_SAVED) ))
      echo "- ix avg query: ${IX_AVG}ms, rg avg query: ${RG_AVG}ms, saving ${RG_SAVED}ms/query"
      echo "- Break-even: **~${BREAK_EVEN} queries** (index build pays back after this many searches)"
    else
      echo "- Insufficient data for break-even calc"
    fi
  fi
  echo ""

  echo "## Indexed Query Latency (p50/p95 ms)"
  echo ""
  echo "| Query Class | Tool | p50 (ms) | p95 (ms) | Matches |"
  echo "|-------------|------|----------|----------|---------|"
  [ -f "$LATENCY_FILE" ] && sort -t'|' -k1,1 -k3,3n "$LATENCY_FILE" | \
    while IFS='|' read -r t l m p c; do
      echo "| $l | $t | $m | $p | $c |"
    done
  echo ""

  echo "## Search Correctness vs rg (match counts)"
  echo ""
  echo "| Query | ix | rg | csearch | zoekt | ugrep |"
  echo "|-------|-----|-----|---------|-------|-------|"
  [ -f "$CORRECTNESS_FILE" ] && while IFS='|' read -r l ix_c rg_c cs_c zo_c ug_c; do
    echo "| $l | $ix_c | $rg_c | $cs_c | $zo_c | $ug_c |"
  done < "$CORRECTNESS_FILE"
  echo ""
} > "$R"

cat "$R"
echo ""
echo -e "${GREEN}Benchmark complete.${RESET}"
echo "Report: $R"