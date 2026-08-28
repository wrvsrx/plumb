# Core Syntax

Read this file completely before authoring plumb source. Core is strict and
semantics-neutral: it recognizes source structure but does not assign meaning
to marker names, inline kinds, direct children, or raw payloads.

## Blocks

Ordinary text forms paragraphs. A marked block starts with one backtick, a
nonempty marker, and an optional head:

```plumb
Paragraph text.

`marker Head text
`marker Another head
```

The marker may contain any non-whitespace, non-control Unicode scalar except
backtick, double quote, brackets, and pipe. It is case-sensitive and kept
losslessly.

A block can have indented children. The first child establishes the child
column; sibling children use exactly that column. Canonical formatting uses one
additional space. Dedent returns to an existing outer indentation level.

```plumb
`parent Head
 `child One
 `child Two
`grandchild Three
```

An empty-head marked block may put its first marked/verbatim child after the
head separator on the same physical line. That child's introducer column
establishes the sibling column:

```plumb
`- `- First
   `- Second
```

A nonblank indented plain line immediately following a marked head, without an
intervening blank line, continues that head. Put a blank line before an
indented paragraph child. Only marked blocks can own children.

Two backticks escape the introducer and produce a literal backtick:

```plumb
``marker is literal text
``kind `[content`] is literal text
```

## Document Root

The document is an implicit root owner. It contains one source-ordered sequence
of top-level blocks. Braces are ordinary parsed text and never establish an
ownership site. Semantic profiles may interpret ordinary top-level marker
spellings as document declarations.

```plumb
`= title Document title

Body paragraph.
```

## Direct Ownership And Raw Tails

Every marked block owns one source-ordered sequence of directly indented
children. Declarations and ordinary structural children may interleave; core
does not interpret their marker spellings.

```plumb
`node Head
 `@ intro
 `+ note
 `= level 2
 `note ordinary child

`task A head that wraps
 across lines

 `= created 2026-08-07T09:00:00+08:00

```

A marked block may have one terminal raw tail after all children. Put `|` and
one or more quotes on the owner's introducer column, then indent each nonempty
payload line by the quote count:

```plumb
`rust
 `@ example
|"
 fn main() {}
```

The boundary is an optional singular owner field, not a child. Once it appears,
the owner remains in raw phase and cannot resume parsed structure. An anonymous
raw leaf uses a backtick and one or more quotes and enters raw phase
immediately:

```plumb
`"
 anonymous raw
```

It has no head or children. When raw content needs declarations or ordinary
children, use the explicit transparent `()` marker and a raw tail. Braces,
including the removed postfix `{...}` and `{#id .class key=value}`
spellings, are ordinary current parsed text; use the explicit migration command
for legacy ownership syntax.

## Parsed Inline Elements

A parsed inline element has a nonempty kind and ordered members:

```plumb
`kind[content|@[stable]|+[class]|=[key|value]]
`outer[before `inner[nested] after|child[value]|code"[raw]"]
```

Bare `[` and `]` are always structural: an unescaped opening bracket is legal
only right after an inline kind and an unescaped closing bracket closes the
current element. A member-level `|` is structural. Literal brackets and parsed
member pipes use the single-backtick escape; braces are ordinary text.
Parsed inline elements may cross continuation lines belonging to the same paragraph/head; those
boundaries become soft breaks. Blank lines, dedents, block-only entries, and
EOF remain hard boundaries.

Core does not interpret kinds. For example, `*[text]` and `_[text]` are generic
inline elements unless the official semantic profile explicitly defines them.

The opening `[` is mandatory and the kind must be nonempty; there is no
anonymous element. Every element has a first parsed argument, which may be empty;
`kind[]` therefore contains one empty parsed argument. A bare `` `kind`` is
invalid, and there is no zero-member element.

A parsed inline element uses one envelope with `|`-separated ordered members:

```plumb
`kind[only]
`kind[first|second]
`kind[first|child[value]|"[raw argument]"|code"[raw child]"]
```

The first member is always a parsed argument. In later members, a nonempty kind
followed by `[` is an introducer-elided parsed child, while a kind followed by a
full quote/bracket envelope is a verbatim child. Full unkinded verbatim
envelopes are arguments only after a `|`; use `kind[|"[raw]"]` when the first
parsed argument is empty. Compact quotes inside members remain parsed text;
compact verbatim is available only as an introduced standalone inline.
Arguments and children may interleave after the first argument. Whitespace
remains argument content; only `|` separates members.
Use `` `|`` for a literal pipe inside parsed argument content. Core preserves
source order but does not assign kind-specific meaning or arity.

## Inline Verbatim

Compact inline verbatim starts with a backtick, an optional opaque kind, and a
single double-quoted nonempty raw payload:

```plumb
`"cargo test"
`rust"let x = 1;"
`$"x^2"
```

For an empty payload, multiple opening quotes, or a payload containing a quote,
use a strengthened quote-and-bracket envelope;
the closing bracket must be followed by the same quote count:

```plumb
`"[contains " safely]"
`rust""[contains ]" safely]""
```

Raw content stays on one physical line and is not parsed. Standalone inline
verbatim has no children; use a full element with a verbatim argument when an
owner also needs children.

## Block Raw

An anonymous verbatim block uses a backtick and one or more opening quotes, then
ends the opener line immediately. The quote count declares the number of ASCII
structural spaces on each nonempty raw-body line:

```plumb
`"
 anonymous raw
```

The body ends at the first nonblank line indented less than that margin. After
the structural spaces, preserve payload spaces, tabs, line endings, and
syntax-like text exactly. Internal blank lines need no margin. A trailing blank
line belongs to the payload only when it carries the complete declared margin;
the first marginless trailing blank ends the payload and becomes block layout.
There is no closing fence. An empty anonymous verbatim block is valid.
Named block raw uses a terminal `|` plus quote-run boundary on the owner column. The
quote count declares the raw-body structural margin just as it does for an
anonymous opener. Canonical formatting preserves an existing quote count and
its matching margin without changing raw payload bytes; newly owned raw payloads
default to one quote. A childless owner's boundary is adjacent to its head,
while an owner with children keeps one blank separator before the boundary.

## Avoid Markdown And Djot Assumptions

- Do not write `# heading`, `- item`, fenced code blocks, or Markdown links
  without the plumb backtick introducer and envelopes.
- Do not assume punctuation is globally special.
- Escape literal `[`, `]`, or member-level `|` with a single backtick.
  Braces are ordinary. There is no general backslash escape language.
- Do not turn a syntax error into literal text. Repair the intended structure.
