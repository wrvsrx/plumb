#!/bin/sh
set -eu

grammar_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$grammar_dir"

nix develop .#tree-sitter-plumb -c sh -c '
  set -eu
  tree-sitter generate
  mkdir -p build
  tree-sitter build -o build/plumb.so
'
