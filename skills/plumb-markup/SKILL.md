---
name: plumb-markup
description: Write, edit, review, or convert strict plumb (.plumb) documents using the released core syntax and official semantic profile. Use for plumb blocks, inline elements, direct children, raw content, headings, lists, definitions, metadata, links, images, file attachments, citations, quotes, inline styles, tasks, events, anchors, references, or documents consumed by the plumb toolchain.
---

# Plumb Markup

Treat `.plumb` as strict plumb, not Markdown or Djot. A special spelling that
starts parsing must be complete and valid; never rely on fallback to literal
text.

## Workflow

1. Read `references/core-syntax.md` completely before changing plumb source.
2. Read `references/standard-semantics.md` completely when standard semantic
   structures are involved.
3. Preserve nearby indentation, direct declarations, ids, raw bytes, and
   reference spelling unless the requested change requires modifying them.
4. Validate edited documents with repository checks or `plumb export FILE`.
5. Use `plumb-edit` owned mutations in the plumb repository; do not construct
   authoritative replacement syntax in adapters.

Inside the repository, `docs/reference/core-syntax.plumb`,
`docs/reference/standard-semantics.plumb`, and
`docs/reference/diagnostics.plumb` override this portable projection.

## Core Rules

- A nonblank physical line starts one block. Newline separates blocks;
  indentation gives a parsed block direct children. Anonymous and marked parsed
  blocks may own children. There are no continuation lines.
- A marked block starts with backtick plus a nonempty marker. A block marker is
  followed by ASCII space or line ending. A marker immediately followed by `{`
  or an inline quote envelope belongs to an inline owner instead.
- Inline content losslessly preserves Text and ASCII SpaceRun elements. Direct
  spaces separate positional data at the current inline depth. Repeated spaces
  do not create empty data; write `{}` for an explicit empty datum.
- `{content}` is one anonymous group in its parent. `` `kind{content} `` is a
  marked group. Groups recurse and stay on one physical line.
- Use `` `` `` for a literal backtick, `` `{ `` for a literal opening brace,
  and `` `} `` for a literal closing brace. Brackets and pipe are ordinary text.
- Compact inline raw is `` `"raw" `` or `` `kind"raw" ``. Empty compact raw is
  valid. Payloads containing quotes or beginning with `{` use the strengthened
  form `` `"{raw "content"}" ``; increase the opening/closing quote run when
  the payload contains a closing-like sequence.
- Anonymous and marked block raw open with an own-line `` `" `` or
  `` `kind" ``. Every payload line carries one additional structural ASCII
  space, which is stripped; all following bytes are raw. Raw blank lines also
  carry that margin. There is no closing fence and no raw tail.
- The document is an implicit root owner. Direct top-level `=` blocks are
  metadata. Direct `@`, `+`, and `=` children project as id, facet, and property
  declarations under the official profile; core keeps them as generic owners.
- For leaf `=`/`:` blocks, the first datum is the key/term and remaining head
  data form the value/body. With structural children, the complete head is the
  key/term and children form the value/body.
- Use `plumb migrate --from member-envelope-v1` for the former `[]`/`|`,
  continuation-line, quote-count block margin, and raw-tail epoch. Do not emulate
  the converter with global text replacement.

## Standard Spelling

```plumb
`= title Example
`= tags
 `+ guide

`# Heading
 `@ intro

`- List item
`. Ordered item

`- Implement parser
 `+ task
 `@ write-parser
 `= created 2026-09-02T09:00:00+08:00

`- 14:00--15:00 Parser review
 `+ event
 `= date 2026-09-02
 `= timezone +08:00

`: Term Inline definition body.

`: {Term with spaces}
 Definition body.

`table
 `- name    age
  `+ header
 `- {Alice Smith}    10

See `->{guide guide.plumb#intro}, `->"guide.plumb#intro", and `cite{smith2004}.
Use `img{status `={src static/status.png}} for an image.
Use `file{Demo `={src static/demo.mp4}} for an attachment.

Use `*{emphasis}, `!{strong}, `=={mark}, `~{strikeout}, `^{superscript}, and `_{subscript}.
Inline `$"x^2" math.

`rust"
 fn main() {}
```

Use `-` and `.` for list items and direct leaf `+ task` or `+ event` facets.
Letter prefixes such as `t`/`task` and `e`/`event` offer no legacy construct
completion. Use `->` as the sole Link kind: `` `->{label target} ``. Use marked
verbatim `` `->"target" `` when label and target are identical.

`()` is the transparent block/inline container. Inline ownership is written as
`` `(){container `+{notice}} ``. A standard same-file Link is
`` `->{same-file target #intro} ``. `$` on inline or block verbatim is TeX math.

`table` owns direct `-` rows. Nonempty row-head data are compact cells; an empty
row uses direct non-declaration block children as expanded cells. Direct
`+ header` marks leading header rows or expanded row-header cells.
