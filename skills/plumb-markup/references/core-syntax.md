# Core Syntax

Read this file completely before authoring plumb source. Core is strict and
semantics-neutral: it recognizes source structure but does not assign meaning
to marker names, inline kinds, attached elements, legacy attributes, or raw payloads.

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
attribute sublanguage. Document and block groups contain ordinary blocks;
inline groups contain ordinary inline elements.

```plumb
{
  `title Document title
}

`node Head
  {
    `id intro
    `note
    `level 2
  }

`span[text]{`id[intro] `note[] `level[2]}
```

A block group uses its owner's first structural child position. An inline group
must touch the complete closing delimiter. Groups may recursively contain
owners with their own groups. Core does not assign id, facet, property, class,
or key-value meaning to their content.

The old `{#id .class key=value}` slot remains readable only as a migration
source family. Do not author it. `plumb fmt` preserves the source family;
`plumb migrate-attributes` explicitly converts a valid document.

## Parsed Inline Elements

A parsed inline element has a nonempty kind and parsed content:

```plumb
`kind[content]{`id[stable] `class[] `key[value]}
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

Inline verbatim starts with a backtick followed by zero or more double quotes
and an opening bracket. The closing bracket must be followed by the same number
of quotes:

```plumb
`[cargo test]
`"[contains ] safely]"
`""[contains ]" safely]""
```

Increase the quote count only when the raw content contains a closing-like
sequence. Raw content stays on one physical line and is not parsed. An attached
inline group may follow the complete closing delimiter:

```plumb
`[let x = 1;]{`language[rust]}
```

## Verbatim Blocks

A verbatim block starts with a backtick immediately followed by an attached block group and
has no head. Its raw body uses a fixed two-space margin relative to the opener:

```plumb
`{
  `language rust
  `id example
}
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
- Do not add backslash escapes.
- Do not turn a syntax error into literal text. Repair the intended structure.
