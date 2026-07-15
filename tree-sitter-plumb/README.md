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

## Neovim highlighting

`queries/highlights.scm` uses Neovim's standard tree-sitter capture names and
only highlights the generic core CST. It does not interpret marker or inline
kind values as headings, list items, tasks, emphasis, or links.

Build the parser explicitly through the locked Nix environment:

```sh
./tree-sitter-plumb/build-parser.sh
```

The ignored output is written to `tree-sitter-plumb/build/plumb.so`. Re-run the
script after changing the grammar, scanner, or locked toolchain.

The optional project configuration in `dev/nvim.lua` only loads the prebuilt
parser and this query, registers the `plumb` filetype, and starts tree-sitter
highlighting for `*.plumb` buffers. It does not invoke Nix or compile code
during Neovim startup. Enable it locally with an ignored symlink:

```sh
ln -s dev/nvim.lua .nvim.lua
```

Enable trusted project-local configuration in your global Neovim `init.lua`:

```lua
vim.o.exrc = true
```

Then start Neovim in this repository and approve `.nvim.lua` with `:trust` when
prompted. Project-local configuration can execute arbitrary code, so review it
before granting trust. Use `:InspectTree` to inspect the CST and `:Inspect` to
see the capture under the cursor.

The query can be checked outside Neovim with the tracked fixture:

```sh
nix develop .#tree-sitter-plumb -c tree-sitter query \
  -p tree-sitter-plumb \
  tree-sitter-plumb/queries/highlights.scm \
  tree-sitter-plumb/test/fixtures/highlights.plumb
```
