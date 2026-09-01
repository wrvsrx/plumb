# plumb-syntax-legacy-v2

Frozen syntax-only reader for the member-envelope/pipe-argument epoch at
commit `73a154798acd66ffc31e289bdcd0e648dfc8555a`.

This crate exists only as input to `member-envelope-v1` migration. Do not add
current syntax, semantic analysis, formatting, or editor behavior here. Fixes
are limited to correctness and safety problems that affect reading this frozen
epoch; migration policy belongs in `plumb-migrate`.
