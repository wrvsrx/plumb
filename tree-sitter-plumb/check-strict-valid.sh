#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
grammar_dir="$repo_root/tree-sitter-plumb"
corpus="$repo_root/crates/plumb-core/tests/fixtures/strict-parser.json"
case_dir=$(mktemp -d)
paths_file="$case_dir/paths.txt"

cleanup() {
  rm -rf -- "$case_dir"
}
trap cleanup EXIT

index=0
while IFS=$'\t' read -r encoded_name encoded_source; do
  index=$((index + 1))
  name=$(printf '%s' "$encoded_name" | base64 --decode)
  case_file="$case_dir/case-$index.plumb"
  printf '%s' "$encoded_source" | base64 --decode >"$case_file"
  printf '%s\n' "$case_file" >>"$paths_file"
  printf '%d\t%s\n' "$index" "$name"
done < <(
  jq -r '.[] | select(.valid == true) | [(.name | @base64), (.source | @base64)] | @tsv' "$corpus"
)

if ! tree-sitter parse --quiet \
  --config-path "$grammar_dir/config.json" \
  --grammar-path "$grammar_dir" \
  --paths "$paths_file"; then
  tree-sitter parse \
    --config-path "$grammar_dir/config.json" \
    --grammar-path "$grammar_dir" \
    --paths "$paths_file"
  exit 1
fi
