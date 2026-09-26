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

`:` is a definition entry. Without structural children, its first positional
element is the term and all remaining head elements form the inline body. Group a multi-element term.
With children, the complete head is the term and children are the block body.

```plumb
`: Term Inline body.
`: {Term with spaces} Another inline body.

`: Term with spaces
 Definition body.
```

## Metadata and Direct Declarations

Direct top-level `=` blocks are document metadata. Document identity comes
from workspace-relative path, so top-level `@` is unsupported. Direct leaf `+`
declarations preserve document facets with one nonempty plain name; unknown
facets are opaque and do not render as body content.

```plumb
`= title Document title
`= created 2026-09-02T09:00:00+08:00
`= tags
 `+ plumb
 `+ notes
`= author
 `= name Alice
```

A leaf property uses its first positional element as key and all remaining head elements as
value. Group a key containing spaces. With children, the complete head is the
key and children form a null, scalar, `+` sequence, nested `=` map, or one
verbatim value. Metadata `+` is non-rendered sequence data; `-` remains a
rendered list item.

Under an ordinary marked block or inline group, direct `@`, `+`, and `=` owners
project as id, facet, and property declarations. They stay source ordered and
may interleave with ordinary children/content. Semantic positional views skip
recognized direct declarations; unknown marked owners remain visible content.

## Links

`->` is the sole Link inline kind. With one non-declaration element, its rich
source is the derived label and its recursive string value is the target. With
two or more elements, the first is an explicit label and all remaining elements
stringify to one target, preserving whitespace and direct adjacency.

```plumb
`->{guide.plumb}
`->{`!{guide.plumb}}
`->{same-file target #intro}
`->{guide Project Guide.plumb}
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

## Embedded Links

Only parsed `->` links accept the `embed` facet. Other groups receive
`resource.invalid-owner`. The first/rest binding is the same as ordinary links:
one element derives its label; otherwise the first element is the label.

```plumb
`->{{status icon} static/status.png `+{embed}}
`->{{Demo video} static/demo.mp4 `+{embed}}
`->{Podcast https://example.test/stream `+{embed} `={type audio/ogg}}
`->{{} static/decorative.png `+{embed}}
`->{{Download report} reports/final.pdf}
```

Explicit `type` (MIME) takes precedence over case-insensitive path extension
inference. URL query/fragment do not participate. Unknown explicit type falls
back to a link without extension inference. Classification never reads files
or fetches URLs. Images export as Pandoc Image; other embeds export as Link.
Both preserve `data-plumb-facet=embed` and the original explicit type on round
trip. Web supports image/video/audio with link fallback. All embeds are excluded
from document graph edges. Plain downloads need only an ordinary link.
Neither `img`/`file` markers nor facets select resource semantics, and `src` is
not a target property.

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
`!{strong content with spaces}
`!{prefix`*{nested}suffix}
`=={mark}
`~{strikeout}
`^{superscript}
`_{subscript}
`!{}
```

Each is a content container and accepts zero or more visible elements. Direct
declarations become attributes rather than visible style content. Multiple
elements inside a style need no anonymous group.

## Tasks

A root `+ task` facet makes the document itself a task. Its title is document
metadata `title` (fallback: filename), and its body is the details. Top-level
list tasks become children without inheriting closure, focus, or dependencies.
Document tasks have path identity, no root anchor, and do not support `recur`.
Removing the facet preserves properties, focus history, body, and children.

```plumb
`= title Release plan
`+ task
`= depends
 `+ Project Plan.plumb
 `+ other.plumb#review

Release details.
```

A path without a fragment refers only to a document task; `path.plumb#id`
refers to a list task. Paths are relative to the referring file. For multiple
references use separate groups or direct `+` sequence items, preserving spaces
inside each path. `prev` accepts one scalar reference.

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
`priority`, `depends`, and `focused`. Datetimes are RFC 3339. State is derived as ready,
waiting, blocked, done, canceled, or conflicted; no status field or checkbox
syntax exists. `task` as a block marker is generic.

Letter prefixes such as `t`/`task` and `e`/`event` offer no legacy construct
completion. Task/Event completion starts from list-marker context.

## Events

A list item becomes an event through direct leaf `+ event`. The first head
positional element is schedule and all remaining head elements form title, so ordinary multiword
titles need no group.

```plumb
`- 14:00--15:00 Parser review
 `+ event
 `@ review
 `= date 2026-09-02
 `= timezone +08:00
 `= tasks #write-parser
```

Schedules accept a point, `START--END`, or a running `START--` interval; document/ancestor `date` and
`timezone` provide context. Task and event facets conflict on one item.

Events can inherit `event-category` values from directly linked list items or whole plumb
documents (including ordinary documents). Reusable activities are ordinary anchored list items, without an `activity`
property or facet. Explicit `tasks` takes precedence over title links; details and
nested inline links do not participate in accounting. Distinct targets split time
equally before event-category aggregation. Duplicate references count once. Missing
categories remain unclassified; unresolved references make the result incomplete.
An event's own `event-category` overrides event-category attribution without changing the split.
Categories accept nonempty plain text, anonymous grouping and unmarked verbatim,
but not rich values or duplicate declarations. Scalar properties split first : rest, so
`= event-category phd misc` is one event-category. Multiple categories use direct leaf `-`
children under `= event-category`, one full plain value per item. Duplicate values count
once; each item share is divided equally among its categories.

`plumb event check-category --json` checks all selected events without a time window.
`--explicit` checks only event declarations, without resolving references. Exit 0
means all categorized, 1 means missing categories, and 2 means incomplete/invalid.

`plumb event summary --from RFC3339 --to RFC3339` clips events to a half-open
window and supports `--group-by category|item|task` and `--json`.
`plumb event check-timeline` uses the same explicit window and requires exactly
one event covering each instant: exit 0 passes, 1 reports gaps/overlaps, and 2
reports failure/incompleteness. Point events do not provide coverage. Task focus
history and note tags do not define event accounting. Calendar `CATEGORIES` is
only a set; use summary JSON for exact allocated totals.

## Tables

`table` owns direct `-` rows. Every direct row-head positional element is a compact cell;
one or more spaces are one separator, so explicit alignment padding does not
change arity. Group a multiword cell and use `{}` for an empty cell.

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

The accounting declaration is `event-category`; a generic `category` property does not
participate. Whole-document links inherit root event-category; item links do not
fall back to their document. Ordinary documents do not become tasks. CLI command
names and report JSON fields are unchanged.

Editor event-category value completion reuses all valid category sets declared by workspace documents and list items, including items without ids. Both scalar values and direct category-list items support prefix completion; multiword values remain a single category.
