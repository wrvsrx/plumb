# Standard Semantics

Core keeps marker spellings generic. This file defines the official profile.

## Headings and Anchors

One through six `#` characters are heading markers. Add direct `@` when a
heading or other marked owner needs an explicit link target.

```plumb
`# Introduction
 `@ intro
`## Details
```

No implicit anchor is generated from a title.

## Lists and Definitions

`-` and `.` are bullet and ordered list items. Adjacent siblings of the same
marker form one list; nested items form nested lists.

```plumb
`- First
`- Second
 `- Nested

`. First
`. Second
```

`:` is a definition entry. Without structural children, its first datum is the
term and all remaining head data form the inline body. Group a multiword term.
With children, the complete head is the term and children are the block body.

```plumb
`: Term Inline body.
`: {Term with spaces} Another inline body.

`: Term with spaces
 Definition body.
```

## Metadata and Direct Declarations

Direct top-level `=` blocks are document metadata. Document identity comes
from workspace-relative path, so top-level `@` and `+` are unsupported.

```plumb
`= title Document title
`= created 2026-09-02T09:00:00+08:00
`= tags
 `+ plumb
 `+ notes
`= author
 `= name Alice
```

A leaf property uses its first datum as key and all remaining head data as
value. Group a key containing spaces. With children, the complete head is the
key and children form a null, scalar, `+` sequence, nested `=` map, or one
verbatim value. Metadata `+` is non-rendered sequence data; `-` remains a
rendered list item.

Under an ordinary marked block or inline group, direct `@`, `+`, and `=` owners
project as id, facet, and property declarations. They stay source ordered and
may interleave with ordinary children/content. Semantic positional views skip
recognized direct declarations; unknown marked owners remain visible content.

## Links

`->` is the sole Link inline kind. With one non-declaration datum, its rich
source is the derived label and its recursive string value is the target. With
two data, the first is an explicit label and the second stringifies to target.

```plumb
`->{guide.plumb}
`->{`!{guide.plumb}}
`->{same-file target #intro}
`->{{other document} guide.plumb}
`->{{cross-file target} guide.plumb#intro `@{cross-file}}
`->{{Project guide} `"Project Guide.plumb"}
```

The target is omitted from containing plain-text projections. A target with a
scheme or `//` is external; other targets are raw relative filesystem paths.
`#` separates an explicit anchor.

Marked verbatim is the compact derived-label Link spelling:

```plumb
`->"https://example.test/a%20b"
`->"guide.plumb#intro"
`->"../assets/manual draft.pdf"
```

## Images and Attachments

`img` and `file` use visible first positional content and a direct `src`
property group:

```plumb
`img{status icon `={src static/status.png}}
`file{{Demo video} `={src static/demo.mp4}}
```

Local sources are raw relative paths. Export lowers files to portable Pandoc
Links and may enhance supported local video MIME types in the Web viewer.

## Citations

The initial citation profile accepts one plain id:

```plumb
See `cite{smith2004}.
```

Declare CSL JSON bibliography paths in metadata:

```plumb
`= bibliography
 `+ static/library.json
```

Clusters, locators, affixes, and alternate citation modes are not defined.

## Quotes and Inline Styles

`>` is block quote. Its head is the opening paragraph and body children are
subsequent quote blocks.

```plumb
`> Opening quote.
 Second paragraph.
 `> Nested quote.
```

The six standard inline styles are:

```plumb
`*{emphasis}
`!{strong}
`=={mark}
`~{strikeout}
`^{superscript}
`_{subscript}
```

Each requires one positional datum. Use an anonymous group when styled content
contains direct spaces plus nested inline structure.

## Tasks

A list item becomes a task through direct leaf `+ task`:

```plumb
`- Implement parser
 `+ task
 `@ write-parser
 `= created 2026-09-02T09:00:00+08:00
 `= due 2026-09-03T09:00:00+08:00
 `= depends #design Project Plan.plumb#review
```

The complete item head is title and body children are details. Defined fields
include `created`, `due`, `wait`, `done`, `canceled`, `recur`, `prev`,
`priority`, and `depends`. Datetimes are RFC 3339. State is derived as ready,
waiting, blocked, done, canceled, or conflicted; no status field or checkbox
syntax exists. `task` as a block marker is generic.

Letter prefixes such as `t`/`task` and `e`/`event` offer no legacy construct
completion. Task/Event completion starts from list-marker context.

## Events

A list item becomes an event through direct leaf `+ event`. The first head
datum is schedule and all remaining head data form title, so ordinary multiword
titles need no group.

```plumb
`- 14:00--15:00 Parser review
 `+ event
 `@ review
 `= date 2026-09-02
 `= timezone +08:00
 `= tasks #write-parser
```

Schedules accept a point or `START--END`; document/ancestor `date` and
`timezone` provide context. Task and event facets conflict on one item.

## Tables

`table` owns direct `-` rows. Every direct row-head datum is a compact cell;
one or more spaces are one separator, so formatter alignment does not change
arity. Group a multiword cell and use `{}` for an empty cell.

```plumb
`table
 `- name             age
  `+ header
 `- {Alice Smith}    10
 `- Bob              {}
```

An empty-head row uses direct non-declaration block children as expanded cells:

```plumb
`table
 `-
  `+ header
  name
  age
 `-
  {Alice Smith}
  10
```

Direct `+ header` marks leading header rows or expanded row-header cells.
Rows must have one effective column count. Rowspan, colspan, widths, alignment,
table foot, and complex grouping are unsupported.

## Math and Generic Export

`$` on inline or block verbatim is TeX math:

```plumb
Inline `$"x^2" math.

`$"
 E = mc^2
```

`()` is the transparent container and `>` is quote. Other generic marked
blocks export as Divs, marked groups as Spans, verbatim blocks as CodeBlocks,
and inline verbatim as Code. Export emits Pandoc JSON for piping to a Pandoc
writer; unsupported Pandoc import nodes are rejected rather than discarded.
