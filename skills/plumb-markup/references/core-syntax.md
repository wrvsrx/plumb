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
column; sibling children use exactly that column. Use two additional spaces as
the normal style. Dedent returns to an existing outer indentation level.

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
``kind[content] is literal text
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

`task Next-line layout
      {
        `: created 2026-08-07T09:00:00+08:00
      }

`span[text]{`@[intro] `-[note] `:[level 2]}
```

A marked/verbatim block group is separated from its complete header by
horizontal whitespace. A same-line close selects compact form; an opener at
the end of the header selects expanded form, whose close returns to the owner
column. A marked block may instead open an expanded group on the immediately
following structural line. That opener must be deeper than the owner and use
the established continuation column when the head spans lines; its close
returns to the opener column. A blank line breaks this ownership. Verbatim
blocks cannot use the next-line form because their line ending begins raw
payload. An inline group must touch the complete closing delimiter. Groups may
recursively contain owners with their own groups. Core does not assign id,
facet, property, class, or key-value meaning to their content.

The removed `{#id .class key=value}` spelling is not part of current syntax.
Do not author it. Ordinary parsing and `plumb fmt` reject it.

## Parsed Inline Elements

A parsed inline element has a nonempty kind and parsed content:

```plumb
`kind[content]{`@[stable] `-[class] `:[key value]}
`outer[before `inner[nested] after]
```

An opening bracket inside parsed content is ordinary text. An unescaped closing
bracket closes the current element; write a literal closing bracket as `` `] ``.
Attached groups must touch the complete closing delimiter. Parsed inline elements
may cross continuation lines belonging to the same paragraph/head; those
boundaries become soft breaks. Blank lines, dedents, block-only entries, and
EOF remain hard boundaries.

Core does not interpret kinds. For example, `*[text]` and `_[text]` are generic
inline elements unless an extension explicitly defines them.

The opening `[` is mandatory. Even an empty inline element is written as
`kind[]`; a bare `` `kind`` is invalid.

## Inline Verbatim

Compact inline verbatim starts with a backtick, an optional opaque kind, and a
double-quoted raw payload:

```plumb
`"cargo test"
`rust"let x = 1;"
`$"x^2"
```

When the payload contains a quote, use a strengthened quote-and-bracket envelope;
the closing bracket must be followed by the same quote count:

```plumb
`"[contains " safely]"
`rust""[contains ]" safely]""
```

Raw content stays on one physical line and is not parsed. An attached inline
group may follow the complete closing delimiter.

## Verbatim Blocks

A verbatim block uses the same backtick, optional opaque kind, and opening quote,
then ends the opener line immediately or after one spaced compact/expanded
attached group. It has no raw head. Its body uses a fixed two-space margin
relative to the opener:

```plumb
`rust" {`@[example]}
  fn main() {
      println!("hello");
  }
```

The body ends at the first nonblank line indented less than that margin. After
the two structural spaces, preserve payload spaces, tabs, line endings, and
syntax-like text exactly. There is no closing fence. An empty verbatim block is
valid.

## Avoid Markdown And Djot Assumptions

- Do not write `# heading`, `- item`, fenced code blocks, or Markdown links
  without the plumb backtick introducer and envelopes.
- Do not assume punctuation is globally special.
- Use a single backtick for active `]`, `{`, or `}` delimiter escapes; there is
  no general backslash escape language.
- Do not turn a syntax error into literal text. Repair the intended structure.
