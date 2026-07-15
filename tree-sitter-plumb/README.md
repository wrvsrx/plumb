# tree-sitter-plumb

The intentionally lenient tree-sitter grammar for plumb editor tooling. The
future hand-written `plumb-core` parser remains the authority for syntax errors
and document acceptance.

## Core-only CST

The grammar represents plumb's core surface syntax with generic nodes such as
`marked_block`, `marker`, `paragraph`, `attributes`, and `inline_element`.
Marker and inline kind tokens remain opaque. It deliberately does not create
nodes such as `heading`, `list_item`, `quote`, `task`, `emphasis`, or `link`.
Those interpretations belong to core lowering or extensions.

`code_block` and `inline_verbatim` are separate syntax nodes because their raw
payloads change the lexical parsing mode. The recovery-only
`incomplete_inline_element` and `incomplete_attributes` nodes keep subsequent
blocks parseable while a document is being edited; they do not make malformed
syntax valid to the strict parser.

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
