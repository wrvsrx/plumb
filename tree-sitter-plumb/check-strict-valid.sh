#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
grammar_dir="$repo_root/tree-sitter-plumb"

cd "$grammar_dir"
tree-sitter generate
cargo test --locked --manifest-path test/strict-alignment/Cargo.toml
