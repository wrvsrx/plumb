# tree-sitter-plumb

The intentionally lenient tree-sitter grammar for plumb editor tooling. The
future hand-written `plumb-core` parser remains the authority for syntax errors
and document acceptance.

## Source and generated files

Edit and commit grammar sources such as:

- `grammar.js`
- `tree-sitter.json`
- future corpus tests and queries
- future hand-written scanners and bindings

Do not edit or commit files generated under `src/`:

- `src/grammar.json`
- `src/node-types.json`
- `src/parser.c`
- `src/tree_sitter/`

The ignore rules name these paths individually so a future hand-written
`src/scanner.c` remains trackable.

## Generate

Run generation through the repository's locked Nix environment:

```sh
cd tree-sitter-plumb
nix develop .#tree-sitter-plumb -c tree-sitter generate
```

Regenerate after every grammar change and before running grammar tests. Never
repair a generated parser by hand; change its source and regenerate it.

Generated parser sources and binary packages are produced for releases in a Nix
sandbox. They are intentionally absent from Git and must be consumed from a
release artifact or regenerated locally.
