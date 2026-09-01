; Core block and inline owners. Marker meanings remain opaque.
(introducer) @punctuation.delimiter
(introducer_escape) @string.escape
(brace_escape) @string.escape
(marker) @keyword
(inline_kind) @keyword
(verbatim_kind) @keyword

[
  "{"
  "}"
] @punctuation.bracket

(verbatim_open) @punctuation.delimiter

((inline_verbatim (raw_text) @markup.raw)
  (#set! priority 90))
(verbatim_block (raw_text) @markup.raw.block)

[
  (incomplete_marked_group)
  (incomplete_anonymous_group)
] @error
