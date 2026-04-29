#!/usr/bin/env bash
set -uo pipefail

IX="/workspace/ix/target/release/ix"
RG="rg"
GREP="grep"
RESULTS="/tmp/ix-stress-results"
rm -rf "$RESULTS" && mkdir -p "$RESULTS"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; BOLD='\033[1m'; RESET='\033[0m'
sec()  { echo -e "${CYAN}══ $* ══${RESET}"; }
info() { echo -e "${GREEN}  $*${RESET}"; }

# Single-run timer: prints wall seconds to stdout, saves output to $RESULTS/$1.out
run_one() {
    local label="$1"; shift
    local out="$RESULTS/${label}.out"
    local s e
    s=$(date +%s%N)
    "$@" >"$out" 2>/dev/null || true
    e=$(date +%s%N)
    awk "BEGIN{printf\"%.4f\",($e-$s)/1e9}"
}

# Median of 3 runs
run_med3() {
    local label="$1"; shift
    local t1 t2 t3
    t1=$(run_one "${label}_1" "$@")
    t2=$(run_one "${label}_2" "$@")
    t3=$(run_one "${label}_3" "$@")
    # Median: sort 3 values, pick middle
    echo "$t1" "$t2" "$t3" | tr ' ' '\n' | sort -n | head -2 | tail -1
}

count_lines() { [ -s "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }
count_files() { [ -s "$1" ] && cut -d: -f1 "$1" | sort -u | wc -l | tr -d ' ' || echo 0; }

# ═══════════════════════════════════════
#  CORPORA
# ═══════════════════════════════════════
sec "Generating corpora"

SELF="/workspace/ix/src"

LARGE="/tmp/ix-stress/large"
rm -rf "$LARGE" && mkdir -p "$LARGE"
for i in 1 2 3 4 5; do
    python3 -c "
import random; random.seed($i)
lines = []
for _ in range(50000):
    r = random.random()
    if r < 0.04:   lines.append('fn {}_{}() -> Result<(), Error> {{'.format(random.choice(['handle','process','validate','transform','compute']), random.choice(['request','response','input','output','data'])))
    elif r < 0.08: lines.append('    // TODO: {} the {}'.format(random.choice(['fix','implement','refactor','optimize','review']), random.choice(['handler','parser','validator','transform','pipeline'])))
    elif r < 0.12: lines.append('    Err(Error::{})'.format(random.choice(['InvalidInput','Timeout','NotFound','PermissionDenied','Internal'])))
    elif r < 0.20: lines.append('    let {} = self.{}_{}()?;'.format(random.choice(['result','value','data','item','entry']), random.choice(['fetch','get','read','load','find']), random.choice(['by_id','by_name','all','latest','first'])))
    elif r < 0.30: lines.append('    tracing::{}!(\"{} {}\"'.format(random.choice(['info','debug','warn','error','trace']), random.choice(['processing','handling','validating','starting','completing']), random.choice(['request','task','job','operation','step'])))
    else:          lines.append('    let x = {};'.format(random.randint(0,9999)))
    if random.random() < 0.0008: lines.append('    // SAFETY: we hold the lock so no other thread can access')
    if random.random() < 0.0005: lines.append('    unsafe { std::ptr::copy_nonoverlapping(src, dst, len) }')
    if random.random() < 0.0004: lines.append('    pub fn execute(&mut self, plan: &QueryPlan, options: &QueryOptions) -> Vec<Match> {')
    if random.random() < 0.0004: lines.append('    // SAFETY: mmap is valid for the lifetime of the Reader')
    if random.random() < 0.0003: lines.append('    #[warn(clippy::pedantic)]')
print('\n'.join(lines))
" > "$LARGE/file_${i}.rs"
done
info "Large: $(du -sh "$LARGE" | cut -f1), $(find "$LARGE" -type f | wc -l) files"

MANY="/tmp/ix-stress/many"
rm -rf "$MANY" && mkdir -p "$MANY"
python3 -c "
import os, random; random.seed(42)
for i in range(2000):
    ext = random.choice(['.ts', '.js', '.rs', '.py', '.go'])
    lines = []
    for _ in range(50):
        lines.append('function handle_{}() {{ console.log(\"{}\"); }}'.format(random.choice(['click','hover','submit','load','error']), random.choice(['processing','done','failed','started','retrying'])))
        if random.random() < 0.03: lines.append('  // SAFETY: we hold the lock so no other thread can access')
        if random.random() < 0.03: lines.append('  pub fn execute(&mut self, plan: &QueryPlan, options: &QueryOptions) -> Vec<Match> {')
        if random.random() < 0.03: lines.append('  #[warn(clippy::pedantic)]')
    with open('/tmp/ix-stress/many/{:04d}{}'.format(i, ext), 'w') as f:
        f.write('\n'.join(lines) + '\n')
"
info "Many: $(du -sh "$MANY" | cut -f1), $(find "$MANY" -type f | wc -l) files"

DEEP="/tmp/ix-stress/deep"
rm -rf "$DEEP" && mkdir -p "$DEEP"
python3 -c "
import os, random; random.seed(77)
for d in range(500):
    depth = random.randint(1, 6)
    path = '/tmp/ix-stress/deep/' + '/'.join(['l{}_d{}'.format(random.randint(1,6), d) for _ in range(depth)])
    os.makedirs(path, exist_ok=True)
    for f in range(random.randint(1, 3)):
        ext = random.choice(['.rs', '.py', '.go', '.ts', '.c'])
        with open(os.path.join(path, 'm{}{}'.format(f, ext)), 'w') as fh:
            lines = []
            for _ in range(40):
                lines.append('fn {}() {{ }}'.format(random.choice(['init','run','test','main'])))
            lines.append('    // SAFETY: we hold the lock so no other thread can access')
            lines.append('    pub fn execute(&mut self, plan: &QueryPlan, options: &QueryOptions) -> Vec<Match> {')
            fh.write('\n'.join(lines) + '\n')
"
info "Deep: $(du -sh "$DEEP" | cut -f1), $(find "$DEEP" -type f | wc -l) files"

MIXED="/tmp/ix-stress/mixed"
rm -rf "$MIXED" && mkdir -p "$MIXED"
python3 -c "
import os, random; random.seed(55)
for i in range(200):
    ext = random.choice(['.rs', '.py', '.go', '.ts', '.c', '.md', '.toml', '.json', '.yaml'])
    with open('/tmp/ix-stress/mixed/code_{}{}'.format(i, ext), 'w') as f:
        lines = []
        for _ in range(80):
            lines.append('fn {}() {{ }}'.format(random.choice(['init','run','test','main'])))
        lines.append('    // SAFETY: we hold the lock so no other thread can access')
        lines.append('    pub fn execute(&mut self, plan: &QueryPlan, options: &QueryOptions) -> Vec<Match> {')
        f.write('\n'.join(lines) + '\n')
for i in range(20):
    with open('/tmp/ix-stress/mixed/binary_{}.bin'.format(i), 'wb') as f:
        f.write(os.urandom(random.randint(2048, 20480)))
        f.write(b'execute_plan_options_query_match_result_handler\n')
        f.write(os.urandom(random.randint(2048, 20480)))
"
info "Mixed: $(du -sh "$MIXED" | cut -f1), $(find "$MIXED" -type f | wc -l) files"

# ═══════════════════════════════════════
#  BUILD INDEXES
# ═══════════════════════════════════════
sec "Building indexes"
echo ""
declare -A CORPUS_DIR
CORPUS_DIR=( [self]="$SELF" [large]="$LARGE" [many]="$MANY" [deep]="$DEEP" [mixed]="$MIXED" )
CORPORA=(self large many deep mixed)

for c in "${CORPORA[@]}"; do
    dir="${CORPUS_DIR[$c]}"
    bt=$(run_one "build_${c}" $IX --build "$dir")
    idx=$(du -sh "$dir/.ix/shard.ix" 2>/dev/null | cut -f1 || echo "N/A")
    disk=$(du -sh "$dir" 2>/dev/null | cut -f1)
    nf=$(find "$dir" -type f -not -path '*/.ix/*' | wc -l)
    info "$c: ${nf} files, ${disk} disk, ${idx} index, build ${bt}s"
done
echo ""

# ═══════════════════════════════════════
#  MAIN BENCHMARK
# ═══════════════════════════════════════
sec "BENCHMARK: ix vs ripgrep vs grep (median of 3)"
echo ""

QUERIES=(
    "rare_lit|lit|SAFETY: we hold the lock"
    "common_lit|lit|fn"
    "ci_lit|ci|execute"
    "rx_simple|rx|err(or|no)"
    "rx_complex|rx|fn \\\\w+_(handler|process|validate)"
    "word|wd|unsafe"
    "nomatch|lit|ZZZ_NOT_FOUND_ANYWHERE_ZZZ"
    "short|lit|pub"
    "ci_rx|ci_rx|safety.*lock"
)

printf "${BOLD}%-8s %-11s %-6s %8s %8s %8s %7s %7s %7s %6s %6s %6s  %-5s%s${RESET}\n" \
    "Corpus" "Query" "Type" "ix(s)" "rg(s)" "grep(s)" "ix#L" "rg#L" "gr#L" "ix#F" "rg#F" "gr#F" "Fast" "Note"
printf "%-8s %-11s %-6s %8s %8s %8s %7s %7s %7s %6s %6s %6s  %-5s%s\n" \
    "─────" "─────" "────" "─────" "─────" "──────" "────" "────" "────" "────" "────" "────" "────" "────"

IX_W=0; RG_W=0; GR_W=0; TOTAL=0; COV_ISSUES=0

for c in "${CORPORA[@]}"; do
    dir="${CORPUS_DIR[$c]}"
    for qdef in "${QUERIES[@]}"; do
        IFS='|' read -r qname qtype qpattern <<< "$qdef"
        TOTAL=$((TOTAL + 1))
        lb="${c}__${qname}"

        case "$qtype" in
            lit)   ix_args=("$qpattern" "$dir"); rg_args=(-n "$qpattern" "$dir"); grep_args=(-rn "$qpattern" "$dir") ;;
            ci)    ix_args=(-i "$qpattern" "$dir"); rg_args=(-i -n "$qpattern" "$dir"); grep_args=(-rin "$qpattern" "$dir") ;;
            rx)    ix_args=(--regex "$qpattern" "$dir"); rg_args=(-n "$qpattern" "$dir"); grep_args=(-rnE "$qpattern" "$dir") ;;
            ci_rx) ix_args=(--regex -i "$qpattern" "$dir"); rg_args=(-i -n "$qpattern" "$dir"); grep_args=(-rinE "$qpattern" "$dir") ;;
            wd)    ix_args=(-w "$qpattern" "$dir"); rg_args=(-w -n "$qpattern" "$dir"); grep_args=(-rnw "$qpattern" "$dir") ;;
        esac

        ix_t=$(run_med3 "${lb}__ix" "$IX" "${ix_args[@]}")
        rg_t=$(run_med3 "${lb}__rg" "$RG" "${rg_args[@]}")
        grep_t=$(run_med3 "${lb}__grep" "$GREP" "${grep_args[@]}")

        ix_out="$RESULTS/${lb}__ix_3.out"
        rg_out="$RESULTS/${lb}__rg_3.out"
        grep_out="$RESULTS/${lb}__grep_3.out"

        ix_l=$(count_lines "$ix_out"); rg_l=$(count_lines "$rg_out"); grep_l=$(count_lines "$grep_out")
        ix_f=$(count_files "$ix_out"); rg_f=$(count_files "$rg_out"); grep_f=$(count_files "$grep_out")

        winner=$(awk "BEGIN{
            if($ix_t<=$rg_t && $ix_t<=$grep_t){print \"ix\"}
            else if($rg_t<=$ix_t && $rg_t<=$grep_t){print \"rg\"}
            else{print \"grep\"}
        }")
        case "$winner" in
            ix)  IX_W=$((IX_W+1)) ;;
            rg)  RG_W=$((RG_W+1)) ;;
            grep) GR_W=$((GR_W+1)) ;;
        esac

        note=""
        if [ "$ix_f" -gt "$rg_f" ] 2>/dev/null && [ "$rg_f" -gt 0 ]; then
            note="IX>RG!"; COV_ISSUES=$((COV_ISSUES+1))
        fi
        if [ "$ix_l" -gt 0 ] && [ "$rg_l" -gt 0 ]; then
            ratio=$(awk "BEGIN{printf\"%.2f\",$ix_l/$rg_l}")
            ok=$(awk "BEGIN{if($ratio>0.80&&$ratio<1.20)print 1; else print 0}")
            if [ "$ok" = "0" ]; then
                note="${note:+${note} }L:${ratio}"; COV_ISSUES=$((COV_ISSUES+1))
            fi
        fi

        printf "%-8s %-11s %-6s %8.4f %8.4f %8.4f %7d %7d %7d %6d %6d %6d  %-5s%s\n" \
            "$c" "$qname" "$qtype" \
            "$ix_t" "$rg_t" "$grep_t" \
            "$ix_l" "$rg_l" "$grep_l" \
            "$ix_f" "$rg_f" "$grep_f" \
            "$winner" "$note"
    done
    echo ""
done

# ═══════════════════════════════════════
#  SUMMARY
# ═══════════════════════════════════════
sec "SPEED SUMMARY"
echo ""
echo "  Tests:    $TOTAL"
echo "  Wins:     ix=$IX_W  rg=$RG_W  grep=$GR_W"
echo "  Coverage: $COV_ISSUES issues"
echo ""

# ═══════════════════════════════════════
#  INDEXED vs BRUTE-FORCE
# ═══════════════════════════════════════
sec "INDEXED vs BRUTE-FORCE (large corpus)"
echo ""
dir="$LARGE"
for qdef in "rare|SAFETY: we hold the lock" "common|fn" "nomatch|ZZZ_NOT_FOUND_ANYWHERE_ZZZ"; do
    IFS='|' read -r qname qpattern <<< "$qdef"
    ix_idx=$(run_med3 "idxvf_${qname}__ix" "$IX" "$qpattern" "$dir")
    ix_noidx=$(run_med3 "noidx_${qname}__ix" "$IX" --no-index "$qpattern" "$dir")
    rg_t=$(run_med3 "idxvf_${qname}__rg" "$RG" -n "$qpattern" "$dir")
    speedup=$(awk "BEGIN{printf\"%.1f\",$ix_noidx/$ix_idx}")
    printf "  %-10s  indexed=%.4fs  brute=%.4fs  rg=%.4fs  ix_speedup=%sx\n" \
        "$qname" "$ix_idx" "$ix_noidx" "$rg_t" "$speedup"
done
echo ""

# ═══════════════════════════════════════
#  COLD vs WARM
# ═══════════════════════════════════════
sec "COLD vs WARM (large corpus, literal 'fn')"
echo ""
dir="$LARGE"
ix_cold=$(run_one "cold2_ix" "$IX" "fn" "$dir")
rg_cold=$(run_one "cold2_rg" "$RG" -n "fn" "$dir")
ix_warm=$(run_one "warm2_ix" "$IX" "fn" "$dir")
rg_warm=$(run_one "warm2_rg" "$RG" -n "fn" "$dir")
printf "  %-6s  cold=%.4fs  warm=%.4fs  speedup=%.1fx\n" "ix" "$ix_cold" "$ix_warm" "$(awk "BEGIN{printf\"%.1f\",$ix_cold/$ix_warm}")"
printf "  %-6s  cold=%.4fs  warm=%.4fs  speedup=%.1fx\n" "rg" "$rg_cold" "$rg_warm" "$(awk "BEGIN{printf\"%.1f\",$rg_cold/$rg_warm}")"
echo ""

# ═══════════════════════════════════════
#  COVERAGE VERIFICATION
# ═══════════════════════════════════════
sec "COVERAGE: ix vs rg file-level comparison"
echo ""
echo "  Checking if ix and rg find the same files for each query..."
echo ""

mismatches=0
for c in "${CORPORA[@]}"; do
    dir="${CORPUS_DIR[$c]}"
    for qdef in "${QUERIES[@]}"; do
        IFS='|' read -r qname qtype qpattern <<< "$qdef"
        lb="${c}__${qname}"

        ix_out="$RESULTS/${lb}__ix_3.out"
        rg_out="$RESULTS/${lb}__rg_3.out"

        [ -s "$ix_out" ] && cut -d: -f1 "$ix_out" | sort -u > "$RESULTS/${lb}_ix_files.txt" || : > "$RESULTS/${lb}_ix_files.txt"
        [ -s "$rg_out" ] && cut -d: -f1 "$rg_out" | sort -u > "$RESULTS/${lb}_rg_files.txt" || : > "$RESULTS/${lb}_rg_files.txt"

        only_rg=$(comm -13 "$RESULTS/${lb}_ix_files.txt" "$RESULTS/${lb}_rg_files.txt" | wc -l | tr -d ' ')
        only_ix=$(comm -23 "$RESULTS/${lb}_ix_files.txt" "$RESULTS/${lb}_rg_files.txt" | wc -l | tr -d ' ')

        status="OK"
        if [ "$only_rg" -gt 0 ] || [ "$only_ix" -gt 0 ]; then
            status="MISSING ix+$only_ix rg+$only_rg"
            mismatches=$((mismatches + 1))
        fi

        printf "  %-8s %-12s  %s\n" "$c" "$qname" "$status"
    done
done
echo ""
echo "  File-level mismatches: $mismatches"
echo ""

# ═══════════════════════════════════════
#  INDEX OVERHEAD
# ═══════════════════════════════════════
sec "INDEX OVERHEAD"
echo ""
printf "  %-10s %-10s %-10s %-10s\n" "Corpus" "Disk(KB)" "Index(KB)" "Ratio"
printf "  %-10s %-10s %-10s %-10s\n" "─────" "───────" "────────" "─────"
for c in "${CORPORA[@]}"; do
    dir="${CORPUS_DIR[$c]}"
    disk_kb=$(du -sk "$dir" 2>/dev/null | awk '{print $1}')
    idx_kb=$(du -sk "$dir/.ix/shard.ix" 2>/dev/null | awk '{print $1}' || echo 0)
    ratio=$(awk "BEGIN{printf\"%.0f%%\",($idx_kb/($disk_kb+1))*100}")
    printf "  %-10s %-10s %-10s %-10s\n" "$c" "$disk_kb" "$idx_kb" "$ratio"
done
echo ""

sec "DONE"
