# Standard Extensions

Read this file completely when using official semantic structures. Core still
stores these as generic syntax; the standard extensions assign the meanings
below.

## Headings And Anchors

Use one through six `#` characters as the marker. The count is the heading
level. Add an explicit id when the heading must be a link or rename target:

```plumb
`#{#intro} Introduction
`## Details
```

Headings without `{#id}` appear in the outline but are not link targets. Only
the `#id` shorthand creates an anchor; `id=value` does not.

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

Use one headless document-level `meta` block containing only `:` definitions:

```plumb
`meta
  `: title

    Document title

  `: created

    2026-07-20T09:00:00+08:00

  `: tags
    `- plumb
    `- notes

  `: author
    `: name

      Alice
```

Keys must be nonempty plain text without whitespace or inline markup. Values
may be empty/null, one paragraph scalar, a `-` list, a nested `:` map, or one
verbatim block. A paragraph or list-item head containing exactly one inline
verbatim value becomes a literal string. A list item with an empty head may use
children to hold another list, map, scalar, or verbatim value. Do not combine a
nonempty item head with children or mix incompatible child shapes. Only the first valid
top-level `meta` block is document metadata; ordinary definitions remain body
content.

Metadata uses only `-` for list values. A metadata list is an ordered data
sequence rather than a rendered bullet or numbered list, so `.` is unsupported
inside `meta`.

The metadata insertion action creates `title` from the filename stem and
`created` from the current local RFC 3339 timestamp.

## Links

Use `->` as the only link inline kind and put the target in `to`:

```plumb
`->[same-file target]{to="#intro"}
`->[other document]{to="guide.plumb"}
`->[cross-file target]{to="guide.plumb#intro"}
`->[external target]{to="https://example.test"}
```

`link` is a generic inline kind, not a link alias. Local and cross-file anchors
must be explicit. Use relative `.plumb` paths. When a task reference path
contains spaces or unsafe bytes, URI-percent-encode the path segment.

When label and target are identical, inline verbatim with `.->` is the standard
Autolink; its payload is both label and target:

```plumb
`[https://example.test/a%20b]{.->}
`"[https://[2001:db8::1]/]"{.->}
`[guide.plumb#intro]{.->}
`[../assets/manual draft.pdf]{.->}
```

The payload must be nonempty. A target with a scheme or `//` prefix is an
absolute/network URI: validate it as a URI but preserve its source spelling.
Other targets are raw relative filesystem paths resolved from the source
document directory. Relative `.plumb` paths and fragments use
document/explicit-anchor resolution; other relative targets are files.

Do not percent-encode, percent-decode, or normalize raw relative paths. UTF-8,
spaces, `%`, `?`, and other path characters are literal. `#` is the sole
structure separator for an explicit anchor, so a relative filename cannot
contain `#`. Control characters and backslashes remain invalid. Use verbatim
quote strength, rather than payload escaping, when `]` conflicts with the
delimiter. Completion inserts the path verbatim and strengthens the envelope
when needed. Use explicit `->` links for custom labels. `.->` is valid only on
inline verbatim and cannot be combined with `to` or `.$`; other attributes are
preserved.

To create an Autolink, type one backtick in ordinary inline content and choose
`Autolink` from construct completion. Once the `.->` facet exists,
the LSP completes document paths and explicit anchors inside its payload. Bare
or unclosed inline verbatim remains ordinary verbatim and does not offer
Autolink candidates.

## Images

Use the `img` parsed inline kind. Its content is alt content and `src` is a
required nonempty URI reference:

```plumb
Text with `img[status icon]{src="static/status.png"} inline.

`img[]{src="https://example.test/decorative.svg"}
```

Empty alt is valid for a decorative image. Relative sources remain URI
references resolved from the source document directory; encode spaces and
unsafe path bytes. There is no separate block-image spelling: an image-only
paragraph is still a paragraph containing one image. Figure, caption, numbering,
and cross-reference semantics are deferred.

## Citations

The current citation profile accepts exactly one plain id:

```plumb
See `cite[smith2004].
```

Do not add `@`, citation clusters, locators, prefixes, suffixes, or alternate
modes. Those forms are deferred.

## Tasks

A task is a `-` or `.` list item carrying `.task`:

```plumb
`-{.task #write-parser created="2026-07-20T09:00:00+08:00" due="2026-07-21T09:00:00+08:00" depends="#design"} Implement parser
  `note Optional details
```

The block head is the title and child blocks are details. Nested `.task` blocks
form the display tree, but only `depends` creates a dependency edge. Add an
explicit id when another task must reference it.

`.task` on another marker is `task.invalid-owner`. The LSP can convert an
ordinary list item to a task while adding `created`, or add `created` to an
existing task; both use the operation's local RFC 3339 timestamp.
At line start, single-backtick construct completion offers Task, Autolink, and
Link, and creates the Task `created` field from the current local RFC 3339
timestamp. Other ordinary inline contexts offer Autolink and Link. Heading,
ordinary list-item, and inline-verbatim constructs are typed directly.

Defined fields:

- `created`, `due`, `wait`, `done`, and `canceled`: quoted RFC 3339 timestamps.
- `recur`: one positive `PnD`, `PnW`, `PnM`, or `PnY` rule; requires `due`.
- `prev`: one same-file `#id` or cross-file `path.plumb#id` reference.
- `depends`: whitespace-separated references in one quoted value.

State is derived from closure timestamps:

- Neither `done` nor `canceled`: open.
- Only `done`: done.
- Only `canceled`: canceled.
- Both: conflicted and invalid for normal operations.

Do not invent `state`, `status`, `priority`, `scheduled`, or checkbox syntax as
task semantics. Other attributes remain opaque custom data.

Completing an open task adds `done`; canceling adds `canceled`. Completion is
rejected while an open dependency blocks the task. Cancel remains allowed.
Closing a recurring task keeps the closed instance and appends the next one,
advancing `due` and `wait`, assigning a unique id, and setting `prev`.

## Export Semantics

`div` and `span` are transparent standard containers and export without a
redundant `data-plumb-marker`. Verbatim inline/block nodes carrying `.$` are
TeX inline/display math. The math facet and optional `language=tex` are
consumed; other attributes are preserved with Span/Div wrappers. `.$` on a
non-verbatim owner is invalid.

`plumb export` emits Pandoc JSON directly. Standard lowering includes headings,
bullet lists, definition lists, metadata, `->` links, `.->` Autolinks, `img`
images, single-id citations, and task attributes. Generic marked blocks become Divs, generic parsed inline
elements become Spans, verbatim blocks become CodeBlocks, and inline verbatim
becomes Code.

Pipe the result to a Pandoc writer rather than invoking a Pandoc plumb reader:

```sh
plumb export document.plumb | pandoc -f json -t html -o document.html
```

Do not assume quote, table, thematic-break, or `*`/`_` emphasis semantics until
an official extension freezes them.
