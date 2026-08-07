; Core block and inline envelopes. Marker and kind meanings remain opaque.
(introducer) @punctuation.special
(introducer_escape) @string.escape
(bracket_escape) @string.escape
(marker) @keyword
(inline_kind) @keyword

; Parsed inline delimiters.
(inline_element
  "[" @punctuation.bracket
  "]" @punctuation.bracket)

(attached_block_group
  "}" @punctuation.bracket)

(block_group_open) @punctuation.bracket

(attached_inline_group
  "{" @punctuation.bracket
  "}" @punctuation.bracket)

(verbatim_open) @punctuation.delimiter

(verbatim_kind) @label

; Raw payloads are syntax nodes because they change the lexical mode.
((inline_verbatim (raw_text) @markup.raw)
  (#set! priority 90))
(verbatim_block (raw_text) @markup.raw.block)

; Recovery nodes represent incomplete editor input, not valid strict syntax.
[
  (incomplete_inline_element)
] @error
