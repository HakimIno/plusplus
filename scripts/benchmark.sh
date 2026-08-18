#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
result_root="${1:-$repo_root/target/performance}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)"
run_dir="$result_root/$run_id"

mkdir -p "$run_dir"
cd "$repo_root"

{
  echo "run_id=$run_id"
  echo "commit=$(git rev-parse HEAD)"
  echo "rustc=$(rustc --version)"
  echo "cargo=$(cargo --version)"
  echo "os=$(uname -s)"
  echo "arch=$(uname -m)"
  echo "kernel=$(uname -r)"
  echo "working_tree_begin"
  git status --short
  echo "working_tree_end"
} > "$run_dir/environment.txt"

cargo bench -p plusplus-core --bench core_hot_paths --locked -- --noplot \
  2>&1 | tee "$run_dir/criterion.txt"

cp -R "$repo_root/target/criterion" "$run_dir/criterion"
echo "Benchmark artifacts: $run_dir"
