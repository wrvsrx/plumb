---
name: plumb-markup
description: Write, edit, review, or convert strict plumb (.plumb) documents using the released core syntax and standard extensions. Use for plumb blocks, inline elements, attributes, raw content, headings, lists, definitions, metadata, links, images, citations, tasks, references, or documents consumed by the plumb toolchain.
---

# Plumb Markup

Treat `.plumb` as strict plumb, not Markdown or Djot. A special spelling that
starts parsing must be complete and valid; do not rely on fallback to literal
text.

## Workflow

1. Read `references/core-syntax.md` completely before creating or changing
   plumb source.
2. Read `references/standard-extensions.md` completely when the document uses
   headings, lists, definitions, metadata, links, images, citations, tasks, anchors, or
   export semantics.
3. Preserve nearby source style, indentation, attributes, explicit ids, and
   reference spelling unless the requested change requires modifying them.
4. Use only frozen standard spellings. Keep unknown markers and inline kinds
   generic; do not infer Markdown semantics from punctuation.
5. Validate edited documents with the repository's required checks. When no
   repository-specific command exists and `plumb` is available, run
   `plumb export FILE >/dev/null` as a strict parse/export check.

In an editor using `plumb lsp`, completion after a single backtick at line start
offers Task, Autolink, and Link. The Task skeleton includes a current local RFC
3339 `created` timestamp. Other ordinary inline contexts offer Autolink and
Link. Heading, ordinary list-item, and inline-verbatim spellings are typed
directly. Snippet-capable clients receive tab stops.

## Authority

Treat this bundled skill as the portable guide for the release that shipped
it. Inside the plumb source repository, prefer
`docs/reference/core-syntax.plumb`, then
`docs/reference/standard-extensions.plumb` and
`docs/reference/diagnostics.plumb` whenever they conflict with this skill.

## Core Rules

- Preserve strict syntax; never silently rewrite malformed input as ordinary
  text.
- Use spaces for structural indentation. Do not use tabs in indentation.
- Attach attributes directly to their block marker or complete inline
  delimiter. Whitespace before attributes changes their ownership.
- Use explicit `{#id}` anchors. Headings do not generate implicit ids.
- Parsed inline elements may cross valid paragraph/head continuation lines;
  inline verbatim payloads remain on one physical line.
- Do not invent quote, table, thematic-break, emphasis, or strong
  semantics. Generic markers and inline kinds remain generic.

## Standard Spelling

```plumb
`#{#intro} Heading

`- List item
`. Ordered item
`-{.task #write-parser} Implement parser

`div{.notice} Transparent block container
Inline `span[container]{.notice} and `[x^2]{.$} math.

`: Term

  Definition body.

See `->[guide]{to="guide.plumb#intro"}, `[guide.plumb#intro]{.->}, and `cite[smith2004].

Use `img[status icon]{src="static/status.png"} for an image.

Use `[cargo test] for inline raw text.

`{language=rust}
  fn main() {}
```

Use `-` for bullet-list items, `.` for ordered-list items, and `->` as the sole
link inline kind. Use `.->` for a verbatim absolute URI or raw relative path
whose payload is both label and target; relative `.plumb` targets resolve as
documents and other relative targets resolve as files. Use
`img[alt]{src="target"}` for images.
`item` and `link` remain syntactically valid generic names but
have no list or link semantics. Only `-` and `.` items may carry the standard
`.task` facet. `div` and `span` are transparent containers; `.$` on verbatim
inline/block nodes is TeX math.
