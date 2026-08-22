# Core Syntax

Read this file completely before authoring plumb source. Core is strict and
semantics-neutral: it recognizes source structure but does not assign meaning
to marker names, inline kinds, attached elements, or raw payloads.

## Blocks

Ordinary text forms paragraphs. A marked block starts with one backtick, a
nonempty marker, and an optional head:

```plumb
Paragraph text.

`marker Head text
`marker Another head
```

The marker may contain any non-whitespace, non-control Unicode scalar except
backtick, double quote, brackets, and braces. It is case-sensitive and kept
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

## Attached Groups

Every document, marked/verbatim block, parsed inline, or inline verbatim owner
may have one attached group. This is a postfix ownership structure, not an
attribute sublanguage. Expanded document/block groups contain ordinary blocks;
compact block and inline groups contain ordinary inline elements.

```plumb
{
  `: title Document title
}

`node Head {
 `@ intro

 `- note

 `: level 2
}

`task A head that wraps
 across lines
 {
  `: created 2026-08-07T09:00:00+08:00
 }

`note[text]{`@[intro] `-[facets] `:[level 2]}
```

A marked/verbatim block group is separated from its complete header by
horizontal whitespace. The opening brace is the last structure of the complete
head: it can trail any head line — the header line or a wrapped continuation
line — or occupy the final head continuation line on its own. A same-line
close selects compact form; an expanded close returns to the structural column
of the opener's line. The own-line opener follows the deferred-head
continuation rules: it must be adjacent to the previous head line, deeper than
the owner, and use the established continuation column when the head spans
lines; that column also hosts the close and later child siblings, and a blank
line breaks the continuation. `plumb fmt` canonicalizes the placement by head
shape: a single-line head gets the trailing opener on the header line, a
wrapped head gets the own-line opener. Verbatim blocks cannot use a
continuation-line opener because their line ending begins raw payload. An
inline group must touch the complete closing delimiter. Groups may recursively
contain owners with their own groups. Core does not assign id, facet, property,
class, or key-value meaning to their content.

The removed `{#id .class key=value}` spelling is not part of current syntax.
Do not author it. Ordinary parsing and `plumb fmt` reject it.

## Parsed Inline Elements

A parsed inline element has a nonempty kind and parsed content:

```plumb
`kind[content]{`@[stable] `-[class] `:[key value]}
`outer[before `inner[nested] after]
```

Bare `[`, `]`, `{`, and `}` are always structural: an unescaped opening
bracket is legal only right after an inline kind, an unescaped closing bracket
closes the current element, and the braces open or close attached groups.
Literal delimiters use the single-backtick escape in any position.
Attached groups must touch the complete closing delimiter. Parsed inline elements
may cross continuation lines belonging to the same paragraph/head; those
boundaries become soft breaks. Blank lines, dedents, block-only entries, and
EOF remain hard boundaries.

Core does not interpret kinds. For example, `*[text]` and `_[text]` are generic
inline elements unless the official semantic profile explicitly defines them.

The opening `[` is mandatory and the kind must be nonempty; there is no
anonymous element. Even an empty inline element is written as
`kind[]`; a bare `` `kind`` is invalid.

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

Raw content stays on one physical line and is not parsed. An attached inline
group may follow the complete closing delimiter.

## Verbatim Blocks

A verbatim block uses the same backtick, optional opaque kind, and one or more opening quotes,
then ends the opener line immediately or after one spaced compact/expanded
attached group. It has no raw head. The opening quote count declares the number
of ASCII structural spaces on each nonempty raw-body line:

```plumb
`rust" {`@[example]}
 fn main() {
     println!("hello");
 }
```

The body ends at the first nonblank line indented less than that margin. After
the structural spaces, preserve payload spaces, tabs, line endings, and
syntax-like text exactly. Internal blank lines need no margin. A trailing blank
line belongs to the payload only when it carries the complete declared margin;
the first marginless trailing blank ends the payload and becomes block layout.
There is no closing fence. An empty verbatim block is valid. Canonical formatting
uses one quote and one structural space without changing raw payload bytes.

## Avoid Markdown And Djot Assumptions

- Do not write `# heading`, `- item`, fenced code blocks, or Markdown links
  without the plumb backtick introducer and envelopes.
- Do not assume punctuation is globally special.
- Escape literal `[`, `]`, `{`, or `}` with a single backtick; bare delimiters
  are always structural. There is no general backslash escape language.
- Do not turn a syntax error into literal text. Repair the intended structure.
