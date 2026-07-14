# tree-sitter-plumb

The editor-oriented, intentionally lenient tree-sitter grammar for plumb.
The hand-written parser in `plumb-core` remains the syntax authority.

## Development

```sh
tree-sitter generate
tree-sitter test
```

The grammar exposes marked blocks, code blocks, paragraphs, attributes, parsed
inline elements, inline verbatim, and introducer escapes. The current pure-CFG
implementation treats indentation structurally but does not validate that all
siblings use the exact same indentation column. It recognizes exact inline
verbatim quote matching for delimiter lengths one through three. These are
editor recovery tradeoffs, not changes to the plumb language specification.
