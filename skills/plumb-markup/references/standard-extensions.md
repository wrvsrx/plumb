# Standard Extensions

Read this file completely when using official semantic structures. Core still
stores these as generic syntax; the standard extensions assign the meanings
below.

## Headings And Anchors

Use one through six `#` characters as the marker. The count is the heading
level. Add an explicit id when the heading must be a link or rename target:

```plumb
`# Introduction
   {
     `@ intro
   }
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

Use `:` for definition entries. The head is the term and children are its
definition body. Adjacent sibling definitions form a definition list:

```plumb
`: Term

  Definition body.
```

## Document Metadata

Use one attached group at the start of the file. Its marked children are
metadata properties:

```plumb
{
  `: title Document title
  `: created 2026-07-20T09:00:00+08:00
  `: tags
    `- plumb
    `- notes
  `: author
    `: name Alice
}
```

Keys must be nonempty plain text without whitespace or inline markup. Values
may be empty/null, one paragraph scalar, a `-` list, a nested `:` map, or one
verbatim block. A paragraph or list-item head containing exactly one inline
verbatim value becomes a literal string. A list item with an empty head may use
children to hold another list, map, scalar, or verbatim value. Do not combine a
nonempty item head with children or mix incompatible child shapes. The first
document root group is metadata; ordinary definitions remain body content.

Metadata uses only `-` for list values. A metadata list is an ordered data
sequence rather than a rendered bullet or numbered list, so `.` is unsupported
inside metadata.

The metadata insertion action creates `title` from the filename stem and
`created` from the current local RFC 3339 timestamp.

## Links

Use `->` as the only link inline kind and put the target in an attached `to`
property:

```plumb
`->[same-file target]{`:[to #intro]}
`->[other document]{`:[to guide.plumb]}
`->[cross-file target]{`:[to guide.plumb#intro]}
`->[external target]{`:[to https://example.test]}
```

`link` is a generic inline kind, not a link alias. Local and cross-file anchors
must be explicit. A target with a scheme or `//` prefix is an absolute/network
URI. Other `to` values are raw relative filesystem paths resolved from the
source document directory. Do not percent-encode, percent-decode, or normalize
them.

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
delimiter. Completion inserts named Link and Autolink paths verbatim; named
Links preserve parsed content, while Autolinks strengthen the envelope when
needed. Use explicit `->` links for custom labels. The `->` kind
is valid only on inline verbatim and cannot be combined with `to` or `$`; other
attached elements are preserved.

To create an Autolink, type a backtick followed by `-` or `->` and choose
`Autolink` from construct completion; continuing with `->"` narrows the
candidate to Autolink. Once the `->` kind exists, the LSP completes document
paths and explicit anchors inside its payload. A bare backtick, standalone `[`,
or unclosed inline verbatim offers no construct candidates.

## Images

Use the `img` parsed inline kind. Its content is alt content and `src` is a
required nonempty target:

```plumb
Text with `img[status icon]{`:[src static/status.png]} inline.

`img[]{`:[src https://example.test/decorative.svg]}
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
`file[Demo video]{`:[src static/demo.mp4]}
`file[Download report]{`:[src reports/final.pdf]}
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

## Quotes

Use the `>` block marker. Its optional head becomes the first paragraph in
the quote, and its children become the remaining quoted blocks:

```plumb
`> A quoted opening.

  A second quoted paragraph.

  `> A nested quote.
```

Empty and nested quotes are valid. Unconsumed attached elements are preserved through an
inner Pandoc Div because Pandoc BlockQuote has no attribute slot. The `>`
marker itself is consumed; `quote` is not an alias and remains generic.
Do not infer attribution, citation, pull-quote, or presentation semantics.

## Inline Styles

Six single-character parsed inline kinds have standard style semantics:

```plumb
`*[emphasis]
`![strong]
`=[mark]
`~[strikeout]
`^[superscript]
`_[subscript]
```

They export as Pandoc Emph, Strong, a Span carrying `.mark`, Strikeout,
Superscript, and Subscript. Attributes on native Pandoc nodes are preserved
through an outer Span. Import consumes the first Pandoc `.mark` class as the standard
`=` spelling and preserves remaining attached data. `**`, `em`, `strong`, and
`mark` are not aliases and remain generic. Italic and bold are common
presentations of emphasis and strong, not separate standard spellings.

## Tasks

A task uses the specialized `task` marker and has bullet-list-item structure:

```plumb
`task Implement parser
   {
     `@ write-parser
     `: created 2026-07-20T09:00:00+08:00
     `: due 2026-07-21T09:00:00+08:00
     `: depends #design
   }
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
the current local RFC 3339 timestamp. At line start and in ordinary inline
content, a backtick followed by a hyphen or arrow offers Link and Autolink;
continuing with `->[` narrows to Link, while `->"` narrows to Autolink. A
standalone opening bracket offers neither. Heading, ordinary list-item, and
other inline-verbatim constructs are typed directly.

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
   {
     `: depends #local Project A.plumb#build Project B.plumb#test
   }
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
task semantics. Other attached elements remain opaque custom data.

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
`event 14:00--15:00 Parser review
   {
     `@ review
     `: date 2026-07-30
     `: timezone +08:00
     `: tasks #write-parser
   }
```

The first inline text in the head is the schedule; ASCII whitespace separates it
from the nonempty title. Children are details. A schedule start may be
reduced-precision `HH`, `HH:MM`, or `HH:MM:SS`; `YYYY-MM-DDTIME`, which
overrides the date and inherits the timezone; or a self-contained full RFC 3339
timestamp. A start with an offset or `Z` must include seconds and cannot append
an offset to a reduced-precision time. A point contains only the start;
`START--TIME` is a half-open interval whose end inherits the resolved start date
and offset. An end local time crossing below the start advances to the next day;
equal times are invalid. A date or offset inside the schedule applies only to that
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
semantics; they remain opaque attached data. Event authoring does not generate
an explicit id or write UID data.

The `Convert to event` action recognizes a reduced-precision schedule at the
start of a list-item head. The title after its separating whitespace may contain
parsed or verbatim inline markup; conversion removes only the leading schedule
and preserves the remaining inline tree and attached elements.
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

## Export Semantics

`div` and `span` are transparent standard containers and export without a
redundant `data-plumb-marker`. `>` exports as Pandoc BlockQuote. `*`, `!`, `=`,
`~`, `^`, and `_` export as the standard inline styles. Verbatim
inline/block nodes with opaque kind `$` are TeX inline/display math. Other
attached elements are preserved with Span/Div wrappers.

`plumb export` emits Pandoc JSON directly. Standard lowering includes headings,
bullet lists, definition lists, metadata, `->` links and Autolinks, `img`
images, `file` attachments, single-id citations, quotes, inline styles, and visible task states with
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
tables, figures, raw HTML, footnotes, and complex citations, are rejected
rather than silently discarded.

Do not assume table, thematic-break, or presentation-only italic semantics until
an official extension freezes them.
