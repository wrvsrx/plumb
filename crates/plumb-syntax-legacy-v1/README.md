# plumb-syntax-legacy-v1

Frozen syntax-only reader for the attached-group/consecutive-slot epoch at
commit `4ee6f40caeaf3b00fa1cd390e49fae5dedb729d1`.

This crate exists only as input to versioned migration. Do not add current
syntax, semantic analysis, formatting, or editor behavior here. Fixes are
limited to correctness and safety problems that affect reading this frozen
epoch; migration policy belongs in the converter crate.
