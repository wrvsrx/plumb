---
name: plumb-markup
description: Write, edit, review, or convert strict plumb (.plumb) documents using the released core syntax and standard extensions. Use for plumb blocks, inline elements, attributes, raw content, headings, lists, definitions, metadata, links, citations, tasks, references, or documents consumed by plumb-ls, plumb-export, and plumb-notes.
---

# Plumb Markup

Treat `.plumb` as strict plumb, not Markdown or Djot. A special spelling that
starts parsing must be complete and valid; do not rely on fallback to literal
text.

## Workflow

1. Read `references/core-syntax.md` completely before creating or changing
   plumb source.
2. Read `references/standard-extensions.md` completely when the document uses
   headings, lists, definitions, metadata, links, citations, tasks, anchors, or
   export semantics.
3. Preserve nearby source style, indentation, attributes, explicit ids, and
   reference spelling unless the requested change requires modifying them.
4. Use only frozen standard spellings. Keep unknown markers and inline kinds
   generic; do not infer Markdown semantics from punctuation.
5. Validate edited documents with the repository's required checks. When no
   repository-specific command exists and `plumb-export` is available, run
   `plumb-export FILE >/dev/null` as a strict parse/export check.

## Authority

Treat this bundled skill as the portable guide for the release that shipped
it. Inside the plumb source repository, prefer `docs/requirements.plumb`, then
`docs/spec.plumb`, `docs/inline.plumb`, and `docs/extensions.plumb` whenever
they conflict with this skill.

## Core Rules

- Preserve strict syntax; never silently rewrite malformed input as ordinary
  text.
- Use spaces for structural indentation. Do not use tabs in indentation.
- Attach attributes directly to their block marker or complete inline
  delimiter. Whitespace before attributes changes their ownership.
- Use explicit `{#id}` anchors. Headings do not generate implicit ids.
- Keep parsed inline elements on one physical line for compatibility with the
  current released parser.
- Do not invent ordered-list, quote, table, thematic-break, emphasis, or strong
  semantics. Generic markers and inline kinds remain generic.

## Standard Spelling

```plumb
`#{#intro} Heading

`- List item
`-{.task #write-parser} Implement parser

`: Term

  Definition body.

See `->[guide]{to="guide.plumb#intro"} and `cite[smith2004].

Use `[cargo test] for inline raw text.

`{language=rust}
  fn main() {}
```

Use `-` as the sole list marker and `->` as the sole link inline kind. `item`
and `link` remain syntactically valid generic names but have no list or link
semantics.
