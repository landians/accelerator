#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

BENCH_NAME="cache_path_bench"
SAMPLE_SIZE="${SAMPLE_SIZE:-60}"
THRESHOLD="${THRESHOLD:-0.15}"
RUNS="${RUNS:-1}"
REDIS_URL="${ACCELERATOR_BENCH_REDIS_URL:-redis://127.0.0.1:6379}"
LOCAL_ONLY_REDIS_URL="redis://127.0.0.1:0"
LOG_DIR="${REPO_ROOT}/target/bench-logs"

SKIP_TESTS="false"
RUN_BENCH_BEFORE_BASELINE="false"

usage() {
    cat <<'EOF'
One-click benchmark helper for accelerator.

USAGE:
  ./scripts/bench.sh [command] [options]

COMMANDS:
  local             Run tests + local-only bench + regression check (default)
  redis             Run tests + redis bench + regression check
  bench-local       Run local-only bench only
  bench-redis       Run redis bench only
  baseline          Export benchmark baseline json
  regression        Run benchmark regression gate only

OPTIONS:
  --sample-size N   Criterion sample size (default: 60)
  --threshold R     Regression threshold ratio (default: 0.15)
  --runs N          Benchmark repeat count (default: 1)
  --redis-url URL   Redis URL for redis bench mode
  --skip-tests      Skip cargo test step in local/redis commands
  --run-bench       Only for baseline: run local-only bench before export
  -h, --help        Show this help message

EXAMPLES:
  ./scripts/bench.sh
  ./scripts/bench.sh redis --redis-url redis://127.0.0.1:6379
  ./scripts/bench.sh bench-local --runs 3 --sample-size 80
  ./scripts/bench.sh baseline --run-bench
  ./scripts/bench.sh regression --threshold 0.10

ENV:
  SAMPLE_SIZE / THRESHOLD / RUNS / ACCELERATOR_BENCH_REDIS_URL
EOF
}

log() {
    echo "[bench.sh] $*"
}

run_tests() {
    if [[ "${SKIP_TESTS}" == "true" ]]; then
        log "skip tests"
        return
    fi

    log "running cargo test -q"
    (cd "${REPO_ROOT}" && cargo test -q)
}

run_single_bench() {
    local label="$1"
    local redis_url="$2"
    local run_index="$3"
    local ts
    ts="$(date +%Y%m%d-%H%M%S)"
    local log_file="${LOG_DIR}/${label}-${ts}-run${run_index}.log"

    mkdir -p "${LOG_DIR}"
    log "running bench ${label} (run ${run_index}/${RUNS}), log: ${log_file}"
    (
        cd "${REPO_ROOT}" && \
        ACCELERATOR_BENCH_REDIS_URL="${redis_url}" \
        cargo bench --bench "${BENCH_NAME}" -- --sample-size="${SAMPLE_SIZE}"
    ) | tee "${log_file}"
}

run_bench_series() {
    local label="$1"
    local redis_url="$2"

    local i
    for ((i = 1; i <= RUNS; i++)); do
        run_single_bench "${label}" "${redis_url}" "${i}"
    done
}

run_regression() {
    log "running regression gate with threshold=${THRESHOLD}"
    (
        cd "${REPO_ROOT}" && \
        cargo run --bin check_bench_regression -- --threshold "${THRESHOLD}"
    )
}

export_baseline() {
    log "exporting baseline to docs/benchmarks/cache_path_bench.json"
    (cd "${REPO_ROOT}" && cargo run --bin export_bench_baseline --)
}

command="local"
if [[ $# -gt 0 && "${1}" != --* ]]; then
    command="$1"
    shift
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --sample-size)
            SAMPLE_SIZE="$2"
            shift 2
            ;;
        --threshold)
            THRESHOLD="$2"
            shift 2
            ;;
        --runs)
            RUNS="$2"
            shift 2
            ;;
        --redis-url)
            REDIS_URL="$2"
            shift 2
            ;;
        --skip-tests)
            SKIP_TESTS="true"
            shift
            ;;
        --run-bench)
            RUN_BENCH_BEFORE_BASELINE="true"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage
            exit 2
            ;;
    esac
done

case "${command}" in
    local)
        run_tests
        run_bench_series "local-only" "${LOCAL_ONLY_REDIS_URL}"
        run_regression
        ;;
    redis)
        run_tests
        run_bench_series "redis" "${REDIS_URL}"
        run_regression
        ;;
    bench-local)
        run_bench_series "local-only" "${LOCAL_ONLY_REDIS_URL}"
        ;;
    bench-redis)
        run_bench_series "redis" "${REDIS_URL}"
        ;;
    baseline)
        if [[ "${RUN_BENCH_BEFORE_BASELINE}" == "true" ]]; then
            run_bench_series "local-only" "${LOCAL_ONLY_REDIS_URL}"
        fi
        export_baseline
        ;;
    regression)
        run_regression
        ;;
    *)
        echo "unknown command: ${command}" >&2
        usage
        exit 2
        ;;
esac

log "done"
