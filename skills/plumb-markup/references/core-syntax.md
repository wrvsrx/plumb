# Core Syntax

Read this file completely before authoring plumb. Core is strict and
semantics-neutral: marker names remain opaque syntax.

## Blocks

Every nonblank physical line starts one block, except for an immediately
indented plain continuation line. A line not beginning with a block marker is
an anonymous parsed block. A marked block uses one backtick, a nonempty marker,
and optional content separated by ASCII space:

```plumb
ordinary paragraph
`note
`note Head content
```

Newline separates blocks. A more-indented following block becomes a child of
the preceding parsed block. The first child establishes its column; siblings
must match it and dedent must return to an established column. Canonical
indentation adds one ASCII space.

```plumb
parent

 child

`parent Head
 `child One
 `child Two
```

Anonymous and marked parsed blocks can own children. A verbatim block cannot.
An immediately indented plain line without an intervening blank continues the
preceding parsed owner's inline content; its boundary is a SoftBreak that typed
projection treats as one semantic space. A blank ends continuation, so a later
indented plain line is an anonymous child. Write `{}` on its own line when an
explicit empty anonymous block is needed.

## Marker Dispatch

A block marker must be followed by ASCII space or line ending. When a marker is
immediately followed by `{` or by a same-line quote envelope, it belongs to an
inline owner in an anonymous block:

```plumb
`note content
`note{content}
```

The first line is a marked block. The second is an anonymous block containing a
marked inline group.

Markers are nonempty, case-sensitive runs excluding whitespace, controls,
backtick, double quote, and braces. Brackets and pipe are ordinary marker/text
characters in this epoch.

## Inline Contents and Data

Inline contents losslessly preserve Text, ASCII SpaceRun, Group, and Verbatim
elements. One or more direct ASCII spaces separate positional data at the
current depth. Repeated spaces still represent one boundary and never create an
empty datum. Adjacent terms without a direct space form one rich datum.

```plumb
prefix`!{strong}suffix
`row Alice   10   {}
```

`{content}` is an anonymous group and occupies one term in its parent.
`` `kind{content} `` is a marked group. Group-internal spaces do not split the
parent datum.

```plumb
{guide with spaces}
`->{{guide with spaces} {Project Guide.plumb}}
`node{visible `@{stable} `={key value}}
```

Groups may nest but may not cross a physical line. `{}` is an explicit empty
datum. The marker is outside the group and is not its first datum.

## Escapes

Backtick is the sole introducer. Parsed text has three escapes:

```plumb
`` literal backtick
`{ literal opening brace
`} literal closing brace
```

Brackets, pipe, and ordinary double quote need no escaping. Malformed special
entries are parse errors, never fallback text. Parsed tabs and controls are
invalid outside verbatim payloads.

## Inline Verbatim

Compact anonymous or marked inline verbatim uses one quote pair and permits an
empty payload:

```plumb
`"cargo test"
`$"x^2"
`$""
```

When the payload contains a quote or begins with `{`, use the full envelope: a
quote run, opening brace, opaque payload, closing brace, and the same quote run.
Increase quote strength until the payload contains no matching closing-like
sequence.

```plumb
`"{raw containing "quotes"}"
`""{raw containing }" safely}""
```

The quote run followed immediately by `{` always dispatches to full verbatim,
so that spelling is never interpreted as compact raw beginning with a brace.
Inline raw stays on one physical line.

## Block Verbatim

An own-line backtick plus quote opens anonymous block raw. Put an opaque marker
before the quote for marked raw:

```plumb
`"
 anonymous raw

`rust"
 fn main() {}
```

Each payload line starts with the opener indentation plus one structural ASCII
space. Strip exactly that space and preserve every following byte. A raw blank
line must carry the margin. The first line lacking it ends raw and is processed
as a normal block. Empty payload and EOF are valid; there is no closing fence,
raw tail, or quote-count block margin.

Raw cannot own parsed children. Wrap it when declarations are needed:

```plumb
`()
 `@ example
 `= language rust
 `rust"
  fn main() {}
```

## Root and Declarations

The document is an implicit root owning top-level blocks. Core stores every
direct child in source order and does not reserve `@`, `+`, or `=`. The standard
profile projects those direct marked owners as id, facet, and property
declarations. The same projection works for direct marked groups inside an
inline owner.

## Strictness and Recovery

Any syntax error makes the document invalid for semantic analysis and export.
The parser still returns a recovered typed tree, complete diagnostics, and a
lossless token stream that reconstructs every input byte. Block errors recover
at physical lines; group and inline-verbatim errors recover at the current line;
partial dedent recovers at an existing outer column.

Use `plumb migrate --from member-envelope-v1` for the removed bracket member
envelope, pipe separators, delimiter-escaped brackets and pipe, quote-count
block margin, and marked raw-tail syntax.
