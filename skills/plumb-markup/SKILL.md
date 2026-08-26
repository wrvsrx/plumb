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
backtick offers no candidates. At line start, a backtick followed by a nonempty
prefix of `task` offers Task, while a prefix of `event` offers Event. Task
includes a current local RFC 3339 `created` timestamp. At line start and in
ordinary inline content, a backtick followed by a hyphen or arrow offers both
Link and Autolink; continuing with `->[` narrows to Link, while `->"` narrows
to Autolink. A standalone opening bracket offers neither. Heading, ordinary
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
- A marked block may end its children with one raw tail. Put one or more `"` on
  the owner column, then indent every nonempty payload line by the same number
  of structural spaces. Keep the boundary adjacent to the head when the owner
  has no children; keep one blank separator after children.
  Anonymous block raw uses an introducer and quote, has no head or children, and
  starts raw payload immediately.
- The document is an implicit root owner. Direct top-level
  `=` blocks are metadata and direct top-level `+` blocks are document facets;
  they may interleave with body blocks. Document identity comes from its
  workspace-relative path, so do not use a direct top-level `@` block.
- Use direct `@` declaration children for explicit ids.
  Headings do not generate implicit ids.
- Separate ordered inline members with `|`. Arguments and introducer-elided
  children may interleave; spaces remain part of an argument.
- Braces are ordinary parsed text. Do not author removed postfix `{...}`
  ownership or `{#id .class key=value}`; migrate legacy source explicitly.
- Parsed inline elements may cross valid paragraph/head continuation lines;
  inline verbatim payloads remain on one physical line.
- Do not invent table, thematic-break, presentation-only italic, or nonstandard quote
  semantics. Generic markers and inline kinds remain generic.

## Standard Spelling

```plumb
`= title Example
`+ guide

`# Heading
 `@ intro

`- List item
`. Ordered item
`task Implement parser
 `@ write-parser
 `= created 2026-07-20T09:00:00+08:00
`event 14:00--15:00 Parser review
 `@ review
 `= date 2026-07-30
 `= timezone +08:00
 `= tasks #write-parser

`() Transparent block container
 `+ notice
`> A quoted paragraph
Use `*[emphasis], `![strong], `==[mark], `~[strikeout], `^[superscript], and `_[subscript].
Inline `()[container|+[notice]] and `$"x^2" math.

`: Term

  Definition body.

See `->[guide|guide.plumb#intro], `->"guide.plumb#intro", and `cite[smith2004].

Use `img[status icon|=[src|static/status.png]] for an image.
Use `file[Demo video|=[src|static/demo.mp4]] for a file attachment with fallback content.

Use `"cargo test" for inline raw text.

`rust
"
 fn main() {}
```

Use `-` for bullet-list items, `.` for ordered-list items, `task` and `event`
for their specialized bullet-list items, and `->` as the sole
link parsed kind; put its label and target in the first two arguments. Use the `->`
verbatim kind for an absolute URI or raw relative path
whose payload is both label and target; relative `.plumb` targets resolve as
documents and other relative targets resolve as files. Use
`img[alt|=[src|target]]` for images and `file[label|=[src|target]]` for attachments.
`item`, `link`, `**`, `em`, and `strong` remain syntactically valid generic names but
have no list or link semantics. `task` and `event` are mutually exclusive
specialized list-item markers; they are not facets on `-` or `.` items.
`()` is the transparent block/inline container; `>` is a block
quote; `*`, `!`, `==`, `~`, `^`, and `_` are inline styles; `$` on inline
verbatim or a marked raw-tail owner is TeX math.
