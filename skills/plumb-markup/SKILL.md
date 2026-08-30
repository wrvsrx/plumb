---
name: plumb-markup
description: Write, edit, review, or convert strict plumb (.plumb) documents using the released core syntax and official semantic profile. Use for plumb blocks, inline elements, direct children, raw content, headings, lists, definitions, metadata, links, images, file attachments, citations, quotes, inline styles, tasks, events, references, or documents consumed by the plumb toolchain.
---

# Plumb Markup

Treat `.plumb` as strict plumb, not Markdown or Djot. A special spelling that
starts parsing must be complete and valid; do not rely on fallback to literal
text.

## Workflow

1. Read `references/core-syntax.md` completely before creating or changing
   plumb source.
2. Read `references/standard-semantics.md` completely when the document uses
   headings, lists, definitions, metadata, links, images, citations, quotes, inline styles, tasks, events, anchors, or
   export semantics.
3. Preserve nearby source style, indentation, direct declarations, explicit ids, and
   reference spelling unless the requested change requires modifying them.
4. Use only frozen standard spellings. Keep unknown markers and inline kinds
   generic; do not infer Markdown semantics from punctuation.
5. Validate edited documents with the repository's required checks. When no
   repository-specific command exists and `plumb` is available, run
   `plumb export FILE >/dev/null` as a strict parse/export check.

For Pandoc JSON input, `plumb import FILE.json` emits canonical strict plumb.
It supports the official exported profile and rejects Pandoc nodes that have no
standard plumb representation instead of dropping them.

In an editor using `plumb lsp`, construct completion is prefix-sensitive: a bare
backtick offers no candidates. At line start, a
backtick followed by a hyphen offers Task, Event, Link, and Autolink. In
ordinary inline content it offers only Link and Autolink; an arrow offers Link
and Autolink in either context. Continuing with `->[` narrows to Link, while `->"` narrows
to Autolink. Letter prefixes such as `t`/`task` and `e`/`event` offer no
construct candidates; Task includes a current local RFC 3339 `created`
timestamp when selected from the list-marker candidates. A standalone opening bracket offers neither. Heading, ordinary
list-item, and other inline-verbatim spellings are typed directly. In ordinary
inline content, a nonempty prefix of `cite` offers Citation. Inside complete or
recovered `cite[...]`, declared CSL JSON bibliography ids are completed.
Snippet-capable clients receive tab stops.
At the end of a plain-text Event title, completion offers workspace Event titles
matching the current case-sensitive prefix, ranked by descending use count.

Inside complete or recovered owner declarations, completion follows the syntax
owner and offers declared standard facets, property names, and finite values while
suppressing unique properties already present. Custom elements remain allowed.
Link/Image paths and anchors continue to use workspace-aware value completion.

The `Align arguments` code action is offered only when the cursor's maximal run
of direct sibling blocks has the same marker and argument count, has at least
two single-line childless/rawless blocks, and needs padding changes. It aligns
every bare `|` column with locked Unicode display widths, leaves tabs and
multiline heads alone, and keeps at least one ASCII space on each side. Metadata
insertion applies the same policy to its generated `title` and `created` pair.
Structured Task/Event authoring also aligns the direct `=` property run that it
actually mutates in the same edit. IDs, facets, ordinary children, and arity
changes split runs; unrelated head/identity/move operations do not align
existing properties.

## Authority

Treat this bundled skill as the portable guide for the release that shipped
it. Inside the plumb source repository, prefer
`docs/reference/core-syntax.plumb`, then
`docs/reference/standard-semantics.plumb` and
`docs/reference/diagnostics.plumb` whenever they conflict with this skill.

## Core Rules

- Preserve strict syntax; never silently rewrite malformed input as ordinary
  text.
- Use spaces for structural indentation. Do not use tabs in indentation.
- Marked blocks own one source-ordered sequence of directly indented children.
  Canonical child indentation is the owner column plus one. Inline owners place
  arguments and children in one `[]` envelope.
- A marked block may end its children with one raw tail. Put `|` and one or more
  `"` on the owner column, then indent every nonempty payload line by the quote
  count. Keep the boundary adjacent to the head when the owner
  has no children; keep one blank separator after children.
  Anonymous block raw uses an introducer and quote, has no head or children, and
  starts raw payload immediately.
- The document is an implicit root owner. Direct top-level `=` blocks are
  metadata and may interleave with body blocks. Document identity comes from
  its workspace-relative path, and the document has no attribute-class
  container, so direct top-level `@` and `+` blocks are unsupported.
- Use direct `@` declaration children for explicit ids.
  Headings do not generate implicit ids.
- Block heads and paragraphs contain ordered parsed arguments. Separate later
  arguments with a bare `|` at the current inline depth. Direct ASCII spaces at
  argument boundaries are typed padding: semantic consumers trim them while
  lossless source and formatting preserve them. Ordinary ASCII space always
  produces inline space, never an argument boundary. A marked head starts at
  the first ASCII space after its marker; that byte is part of the lossless
  head and is trimmed by its typed first-argument view. A tab cannot open a head.
  Consecutive separators create empty arguments, and literal pipe and boundary
  space are written as `` `| `` and backtick-space respectively.
- Every parsed inline owner starts with a parsed argument, which may be empty. Separate
  later ordered members with `|`; arguments and introducer-elided children may
  then interleave. Verbatim members always use the full quote/bracket envelope;
  compact verbatim is standalone only. Direct boundary padding is trimmed;
  each nested parsed argument is independently viewed through the same trimmed
  projection. Padding around later members does not change verbatim-argument or
  parsed/verbatim-child classification.
- Braces are ordinary parsed text. Do not author removed postfix `{...}`
  ownership or `{#id .class key=value}`; migrate legacy source explicitly.
- Migrate the former space-delimited block association, compact definition,
  and Event heads with `plumb migrate --from head-space-v1`. The converter is
  minimal and idempotent; do not emulate it with global whitespace replacement.
- Migrate legacy specialized `task`/`event` markers with
  `plumb migrate --from task-event-markers-v1`; it creates bullet items with a
  first matching facet and leaves current facet-form source unchanged.
- Parsed inline elements may cross valid paragraph/head continuation lines;
  inline verbatim payloads remain on one physical line.
- Do not invent thematic-break, presentation-only italic, or nonstandard quote
  semantics. Generic markers and inline kinds remain generic.

## Standard Spelling

```plumb
`= title|Example
`= tags
 `+ guide

`# Heading
 `@ intro

`- List item
`. Ordered item
`- Implement parser
 `+ task
 `@ write-parser
 `= created|2026-07-20T09:00:00+08:00
`- 14:00--15:00|Parser review
 `+ event
 `@ review
 `= date|2026-07-30
 `= timezone|+08:00
 `= tasks|#write-parser

`() Transparent block container
 `+ notice
`> A quoted paragraph
Use `*[emphasis], `![strong], `==[mark], `~[strikeout], `^[superscript], and `_[subscript].
Inline `()[container|+[notice]] and `$"x^2" math.

`: Term|Inline definition body.

`: Term

  Definition body.

`table
 `- name  | age
  `+ header
 `- Alice | 10
 `- Bob   | 20

See `->[guide|guide.plumb#intro], `->"guide.plumb#intro", and `cite[smith2004].

Use `img[status icon|=[src|static/status.png]] for an image.
Use `file[Demo video|=[src|static/demo.mp4]] for a file attachment with fallback content.

Use `"cargo test" for inline raw text.

`rust
|"
 fn main() {}
```

Use `-` for bullet-list items, `.` for ordered-list items, and direct leaf
`+ task` or `+ event` facets to give either list-item kind task or event semantics.
Use `->` as the sole
link parsed kind; put its label and target in the first two arguments. Use the `->`
verbatim kind for an absolute URI or raw relative path
whose payload is both label and target; relative `.plumb` targets resolve as
documents and other relative targets resolve as files. Use
`img[alt|=[src|target]]` for images and `file[label|=[src|target]]` for attachments.
`item`, `link`, `**`, `em`, and `strong` remain syntactically valid generic names but
have no list or link semantics. `task` and `event` marker spellings are generic.
A list item carrying both facets is a conflict and produces neither record.
`table` owns direct `-` rows. A nonempty row head uses ordered arguments as
compact cells; an empty row head uses direct `-` children as expanded cells.
Use direct `+ header` on leading header rows or on expanded row-header cells.
Expanded cell children contain ordinary blocks. Keep one effective column count;
rowspan, colspan, widths, alignment, table foot, and complex grouping are unsupported.
`()` is the transparent block/inline container; `>` is a block
quote; `*`, `!`, `==`, `~`, `^`, and `_` are inline styles; `$` on inline
verbatim or a marked raw-tail owner is TeX math.
