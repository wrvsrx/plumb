# Standard Semantics

Read this file completely when using official semantic structures. Core still
stores these as generic syntax; the official semantic profile assigns the meanings
below.

## Headings And Anchors

Use one through six `#` characters as the marker. The count is the heading
level. Add an explicit id when the heading must be a link or rename target:

```plumb
`# Introduction
 `@ intro
`## Details
```

Headings without a direct `@` declaration appear in the outline but are not
link targets.

## Lists And Definitions

Use `-` for bullet-list items and `.` for ordered-list items. Adjacent sibling
items with the same marker form one list; switching markers starts another
list. Nested items form nested lists:

```plumb
`- First
`- Second
  `- Nested

`. First
`. Second
  `. Nested
```

Ordered lists always start at 1. `item` is a generic marker, not a list alias.

Use `:` for definition entries. Without children, the first head argument is
the term and an optional second argument is the inline definition body. With
children, the sole head argument is the term and the children are its body.
Adjacent sibling definitions form a definition list:

```plumb
`: Term|Inline body.

`: Term

  Definition body.
```

## Document Metadata

The document is an implicit root owner. Direct top-level `=` blocks are metadata
properties and may interleave with body blocks; declarations are removed from
the projected body without splitting adjacent body lists. Direct top-level `@`
is unsupported because document identity is its normalized workspace-relative
path. Direct top-level `+` is also unsupported because the document has no
attribute-class container. Both unsupported declarations remain outside the
projected body.

```plumb
`= title|Document title
`= created|2026-07-20T09:00:00+08:00

`= tags
 `+ plumb
 `+ notes

`= author
 `= name|Alice
```

Without children, the first head argument is the key and an optional second
argument is the value. With children, the sole plain head argument is the key
and the children are the value. Internal spaces remain argument content, so keys
and values containing spaces need no extra container; direct boundary spaces are
typed padding. Keys must be nonempty plain text. Values
may be empty/null, one paragraph scalar, a `+` sequence, a nested `=` map, or
one verbatim block. A paragraph or collection-member head containing exactly
one inline verbatim value becomes a literal string. A `+` member with an empty
head may use children to hold another sequence, map, scalar, or verbatim value.
Do not combine a nonempty member head with children or mix incompatible child
shapes. Metadata sequences preserve source order. `:` and `-` remain rendered
definition/list structure; `=` and `+` form non-rendered map/sequence data in a
metadata value. Ordinary owner-level leaf-shaped `+` declarations remain
facets.

Metadata uses only `+` for sequence values. A metadata sequence preserves source
order but does not become a rendered bullet or numbered list, so `-` and `.` are
unsupported inside metadata.

Use `plumb migrate --from head-space-v1` for the former flat-head epoch whose
block associations, compact definitions, and Events used a recognized space as
their positional boundary. It minimally replaces only those typed boundaries
with `|` and leaves already-migrated heads unchanged.

The metadata insertion action creates `title` from the filename stem and
`created` from the current local RFC 3339 timestamp.

## Links

Use `->` as the only link inline kind. Its first argument is the label and its
second argument is the target. Interleaved children do not occupy argument indexes:

```plumb
`->[same-file target|#intro]
`->[other document|guide.plumb]
`->[cross-file target|@[cross-file]|guide.plumb#intro]
`->[external target|https://example.test]
```

`link` is a generic inline kind, not a link alias. Local and cross-file anchors
must be explicit. A target with a scheme or `//` prefix is an absolute/network
URI. Other targets are raw relative filesystem paths resolved from the
source document directory. Do not percent-encode, percent-decode, or normalize
them.

When a heading, task, event, metadata value, or another standard semantic
consumer projects visible plain text, a Link contributes only its label. Its
positional target argument is not part of the containing title, details, or scalar
text. Generic inline elements contribute arguments and ordinary children in source order.
Use `plumb migrate --from attached-v1` for legacy consecutive slots and inline
attached groups; the current parser does not read those spellings.

When label and target are identical, inline verbatim with opaque kind `->` is
the standard Autolink; its payload is both label and target:

```plumb
`->"https://example.test/a%20b"
`->"[https://[2001:db8::1]/]"
`->"guide.plumb#intro"
`->"../assets/manual draft.pdf"
```

The payload must be nonempty and uses the same target classification as a named
Link. Validate absolute/network URIs but preserve their source spelling.
Relative `.plumb` paths and fragments use document/explicit-anchor resolution;
other relative targets are files.

Do not percent-encode, percent-decode, or normalize raw relative paths. UTF-8,
spaces, `%`, `?`, and other path characters are literal. `#` is the sole
structure separator for an explicit anchor, so a relative filename cannot
contain `#`. Control characters and backslashes remain invalid. Use verbatim
quote strength, rather than payload escaping, when `]` conflicts with the
delimiter. Named Link completion escapes backticks, brackets, and member pipes
as required by parsed content while preserving the decoded path; Autolinks
strengthen the verbatim envelope when needed. Use explicit `->` links for custom labels. The `->` kind
is valid only on inline verbatim and cannot be combined with `to` or `$`; other
children are preserved.

To create an Autolink, type a backtick followed by `-` or `->` and choose
`Autolink` from construct completion; continuing with `->"` narrows the
candidate to Autolink. Once the `->` kind exists, the LSP completes document
paths and explicit anchors inside its payload. A bare backtick, standalone `[`,
or unclosed inline verbatim offers no construct candidates.

## Images

Use the `img` parsed inline kind. Its content is alt content and `src` is a
required nonempty target:

```plumb
Text with `img[status icon|=[src|static/status.png]] inline.

`img[|=[src|https://example.test/decorative.svg]]
```

Empty alt is valid for a decorative image. Sources with a scheme or `//` prefix
remain URI references. Other sources are raw relative filesystem paths resolved
from the source document directory: do not percent-encode, percent-decode, or
normalize them. UTF-8, spaces, parentheses, `%`, `?`, and `#` are literal;
control characters, backslashes, and absolute filesystem paths are invalid.
Completion inserts filesystem spelling verbatim, apart from an inline-delimiter
escape when required. There is no separate block-image
spelling: an image-only paragraph is still a paragraph containing one image.
Figure, caption, numbering, and cross-reference semantics are deferred.

## File Attachments

Use the `file` parsed inline kind. Its content is the portable fallback label
and `src` is a required nonempty URI or raw relative filesystem target:

```plumb
`file[Demo video|=[src|static/demo.mp4]]
`file[Download report|=[src|reports/final.pdf]]
```

Export lowers a file attachment to a Pandoc Link so every writer retains a
clickable fallback. The Web viewer enhances indexed local video MIME types with
controls while keeping the fallback link. Other file types remain links. MIME
controls renderer capability only; it does not change file semantics or turn
ordinary Links into media.

## Citations

The current citation profile accepts exactly one plain id:

```plumb
See `cite[smith2004].
```

Do not add `@`, citation clusters, locators, prefixes, suffixes, or alternate
modes. Those forms are deferred.

Declare one or more workspace-contained CSL JSON files with document metadata.
Plain scalar paths and inline-verbatim literal paths are both valid; paths are
resolved from the source document directory:

```plumb
`= bibliography
 `+ static/library.json
```

The LSP offers Citation construct completion for a nonempty `cite` prefix and
completes ids inside complete or recovered `cite[...]`. Hover shows the CSL
summary and definition opens the CSL JSON id field. `plumb site serve` runs
citeproc and refreshes rendered notes when a declared CSL JSON file changes.
When no bibliography is declared, valid citations still export portably but
receive a `citation.unresolved` diagnostic.

## Quotes

Use the `>` block marker. Its optional head becomes the first paragraph in
the quote, and its children become the remaining quoted blocks:

```plumb
`> A quoted opening.

  A second quoted paragraph.

  `> A nested quote.
```

Empty and nested quotes are valid. Unconsumed declaration children are preserved through an
inner Pandoc Div because Pandoc BlockQuote has no attribute slot. The `>`
marker itself is consumed; `quote` is not an alias and remains generic.
Do not infer attribution, citation, pull-quote, or presentation semantics.

## Inline Styles

Six parsed inline kinds have standard style semantics:

```plumb
`*[emphasis]
`![strong]
`==[mark]
`~[strikeout]
`^[superscript]
`_[subscript]
```

They export as Pandoc Emph, Strong, a Span carrying `.mark`, Strikeout,
Superscript, and Subscript. Attributes on native Pandoc nodes are preserved
through an outer Span. Import consumes the first Pandoc `.mark` class as the standard
`==` spelling and preserves remaining children. `**`, `em`, `strong`, and
`mark` are not aliases and remain generic. Italic and bold are common
presentations of emphasis and strong, not separate standard spellings.

## Tasks

A task uses the specialized `task` marker and has bullet-list-item structure:

```plumb
`task Implement parser
 `@ write-parser
 `= created|2026-07-20T09:00:00+08:00
 `= due|2026-07-21T09:00:00+08:00
 `= depends|#design
 `note Optional details
```

The block head is the title and child blocks are details. Nested task blocks
form the display tree, but only `depends` creates a dependency edge. Add an
explicit id when another task must reference it.

The LSP can convert an ordinary list item to the `task` marker while adding
`created`, or add `created` to an existing task; both use the operation's local
RFC 3339 timestamp.
Construct completion is prefix-sensitive. A bare backtick offers no candidates.
At line start, a backtick followed by a nonempty prefix of `task` offers Task,
while a prefix of `event` offers Event. It creates the Task `created` field from
the current local RFC 3339 timestamp. The Task replacement uses canonical
owner-column-plus-one child indentation. The LSP projects absolute indentation for
`asIs` clients and owner-relative indentation for `adjustIndentation` clients,
so applying either form produces the same source. At line start
and in ordinary inline content, a backtick followed by a hyphen or arrow offers
Link and Autolink;
continuing with `->[` narrows to Link, while `->"` narrows to Autolink. A
standalone opening bracket offers neither. Heading, ordinary list-item, and
other inline-verbatim constructs are typed directly.
At the end of a plain-text Event title after its schedule, the LSP completes
workspace Event titles using the current case-sensitive title prefix. It deduplicates
titles, ranks them by descending occurrence count then source spelling, and returns
at most 50. An empty prefix offers the most frequent titles. The edit replaces only
the title prefix, never the schedule; an identical nonempty title is omitted.

Defined fields:

- `created`, `due`, `wait`, `done`, and `canceled`: RFC 3339 timestamp properties.
- `recur`: one positive `PnD`, `PnW`, `PnM`, or `PnY` rule; requires `due`.
- `prev`: one same-file `#id` or cross-file `path.plumb#id` reference.
- `priority`: a signed 32-bit integer; larger values have higher priority.
- `depends`: references in one scalar value. Whitespace after each reference id
  separates entries; whitespace before `.plumb#id` belongs to the path.

Task references use raw relative paths. Do not percent-encode, percent-decode,
or normalize them. UTF-8, spaces, parentheses, `%`, and ordinary path
characters are literal. Control characters, backslashes, absolute paths, and
`#` inside a path are invalid. The `.plumb#` sequence separates a cross-file
path from its explicit id:

```plumb
`task Review
 `= depends|#local Project A.plumb#build Project B.plumb#test
```

Datetime fields must use RFC 3339 property values. An invalid value produces
`task.invalid-datetime` and does not participate in task state,
queries, or operations. `task.missing-due-for-recur` applies only when the
`due` property is absent, not when it is present but invalid.
An invalid `priority` produces `task.invalid-priority` and does not participate
in sorting or task CEL facts. Valid priorities are CEL integers; missing or
invalid priorities are CEL null rather than the sorting default below.

Document-local closure state is derived from closure timestamps:

- Neither `done` nor `canceled`: open.
- Only `done`: done.
- Only `canceled`: canceled.
- Both: conflicted and invalid for normal operations.

Do not invent `state`, `status`, `scheduled`, or checkbox syntax as
task semantics. Other declaration and ordinary children remain opaque custom data.

`plumb task` and the Web task view project each document as a virtual structural
node in the workspace task forest. It has no language syntax or editable
file-level priority. Every task and document node has a default own priority of
zero; its effective priority is the maximum of its own priority and the
effective priorities of its direct children. Positive descendants can promote
ancestors, while negative descendants cannot demote them.
Effective priority also propagates from a dependent task to each still-open
dependency, transitively. The propagated value then promotes the dependency's
ancestors and document through the same maximum rule. Closed dependencies do
not receive propagated priority, and cycles converge because propagation only
raises values to a maximum already present in the projected task set.

Sorting recursively reorders only complete sibling subtrees, so documents and
task subtrees remain contiguous. `plumb task` sorts by descending effective
priority, earliest subtree due, then source order. The Web view offers source,
priority, due, and relevance sorts, aggregating the maximum effective priority,
earliest RFC 3339 due instant, or maximum fuzzy score through each subtree.
Filtering first defines the projected result forest. Only retained tasks
contribute priority, due, and relevance to ancestor and document aggregates;
hidden tasks cannot change the result order. A task query limit extends through
the current document rather than splitting its tree.

Workspace queries, LSP hover, and the Web task view derive one mutually
exclusive workflow state with this precedence: `conflicted` when both closure
timestamps exist; `done` or `canceled` when only the corresponding timestamp
exists; `waiting` for an open task with a future wait; `blocked` for an open
task with no future wait and an open dependency; otherwise `ready`. A task with
both a future wait and an open dependency is `waiting`, while ordered
`wait_reasons` still contains both `time` and `dependency` and the independent
`blocked` fact remains true. In task CEL, `actionable` is equivalent to
`state == "ready"`.

Completing an open task adds `done`; canceling adds `canceled`. Completion is
rejected while an open dependency blocks the task. Cancel remains allowed.
Closing a recurring task keeps the closed instance and appends the next one,
advancing `due` and `wait`, assigning a unique id, and setting `prev`.
Status operations format only the complete task subtree; a following sibling
provides spacing context without entering the edit or losing indentation.

Export prefixes the first task paragraph with a visible closure marker: `☐` for
open, `☒` for done, `⊘` for canceled, and `⚠` for conflicted. `☐` and `☒`
follow Pandoc's task-list convention so supporting writers produce checkboxes.
The task id, marker identity, and fields remain on a Span around the title; child
blocks remain subsequent blocks in the same list item.

## Events

An event uses the specialized `event` marker and has bullet-list-item structure:

```plumb
`event 14:00--15:00|Parser review
 `@ review
 `= date|2026-07-30
 `= timezone|+08:00
 `= tasks|#write-parser
```

The head must contain exactly two arguments: a nonempty plain schedule and a
nonempty title. Children are details; the event recognizer does not rescan
whitespace to find the boundary. A schedule start may be
reduced-precision `HH`, `HH:MM`, or `HH:MM:SS`; `YYYY-MM-DDTIME`, which
overrides the date and inherits the timezone; or a self-contained full RFC 3339
timestamp. A start with an offset or `Z` must include seconds and cannot append
an offset to a reduced-precision time. A point contains only the start;
`START--END` is a half-open interval. End accepts `TIME` or
`YYYY-MM-DDTTIME`, inherits the resolved start offset, and cannot carry `Z` or
a numeric offset. A time-only end uses the start date and advances to the next
day when its local time is earlier than the start. A dated end uses its explicit
date without rollover. The resolved end must be strictly later than the start.
A date or offset inside the schedule applies only to that
event and does not propagate to descendants. Event `date` and numeric-offset
`timezone` otherwise override same-named metadata scalar/literal values; an RFC
3339 metadata date also supplies its offset. Old `at`, `start`, and `end`
properties have no event semantics. `tasks` uses the same raw same-file/cross-file reference-list spelling as
task `depends`, but creates associations rather than dependencies. Targets must
be explicit task ids. Never infer events from task timestamps.

Metadata `date` and `timezone` establish the document-root event context.
Same-named properties on any marked block override that block and its descendant
subtree; the nearest ancestor wins, and dedenting restores the parent context.
An event's own overrides therefore also propagate to nested events. An invalid
explicit override never falls back to an ancestor value.

Source `uid` and `when` properties and metadata `event-uids` have no event
semantics; they remain opaque direct declarations. Event authoring does not generate
an explicit id or write UID data.

The `Convert to event` action recognizes a reduced-precision schedule at the
start of a list-item head. The title after its separating whitespace may contain
parsed or verbatim inline markup; conversion removes only the leading schedule
and preserves the remaining inline tree and ordered members.
An event needs an explicit id only when referenced. A shorthand without an
explicit date or timezone inherits valid document metadata
before falling back to the operation's local date and offset.
Trailing `--` is an authoring-only inferred end. Conversion takes the start of
the immediately following sibling list-item shorthand and writes an explicit
interval schedule. Batch inference requires both siblings in the selection; a
single-item conversion may inspect its unselected next sibling. An intervening
block breaks the chain, and a final trailing-`--` item remains unconverted until
it has a following shorthand or an explicit end.
`event` and `task` are mutually exclusive specialized list-item markers and
cannot be combined.
The initial profile has no all-day, floating-time, recurrence, reminder,
attendee, alarm, external-iCalendar import, or CalDAV semantics.

`plumb event export-vdir --output DIR` writes a managed read-only vdir with one
VEVENT per file. Events without a valid resolved time cannot export. The UID is
derived deterministically from workspace-relative path, schedule, and title.
Plumb source remains authoritative; khal should configure the generated calendar
with `readonly = true`.

## Tables

Use `table` with an optional inline caption. Its direct `-` children are rows.
A nonempty row head uses ordered arguments as compact inline cells; an empty row
head uses direct `-` children as expanded cells:

```plumb
`table
 `- name  | age
  `+ header
 `- Alice | 10
 `- Bob   | 20
```

Direct ASCII spaces at compact argument boundaries are padding, so source can
align columns without changing cell values. Consecutive separators create empty
cells. A direct `+ header` marks leading header rows. On an expanded body row,
the same facet marks a row-header cell. Expanded cell heads form their first
paragraph and ordinary children form later blocks; an empty head and body form
an empty cell. Do not mix compact and expanded cells in one row, and keep one
effective column count. Rowspan, colspan, alignment, widths, table foot, and
complex head/body grouping are unsupported.

Export emits a Pandoc Table. Import preserves the supported caption,
attributes, header rows, row-header prefix, and inline/rich/empty cells.

## Export Semantics

`()` is the transparent standard block/inline container and exports without a
redundant `data-plumb-marker`. `>` exports as Pandoc BlockQuote. `*`, `!`, `==`,
`~`, `^`, and `_` export as the standard inline styles. Verbatim
inline nodes with opaque kind `$`, and marked raw-tail owners with marker `$`,
are TeX inline/display math. Other declarations are preserved with Span/Div wrappers.

`plumb export` emits Pandoc JSON directly. Standard lowering includes headings,
bullet lists, definition lists, metadata, `->` links and Autolinks, `img`
images, `file` attachments, single-id citations, quotes, inline styles, tables, and visible task states with
task data. Generic marked blocks become Divs, generic parsed inline
elements become Spans, verbatim blocks become CodeBlocks, and inline verbatim
becomes Code.

Pipe the result to a Pandoc writer rather than invoking a Pandoc plumb reader:

```sh
plumb export document.plumb | pandoc -f json -t html -o document.html
```

`plumb import` performs the reverse conversion for the supported exported
profile and emits canonical strict plumb. Feed other source formats through
Pandoc JSON first. Nodes without a standard plumb representation, such as
figures, raw HTML, footnotes, and complex citations, are rejected
rather than silently discarded.

Do not assume thematic-break or presentation-only italic semantics.
