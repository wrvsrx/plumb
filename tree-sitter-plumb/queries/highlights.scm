; Core block and inline envelopes. Marker and kind meanings remain opaque.
(introducer) @punctuation.special
(introducer_escape) @string.escape
(bracket_escape) @string.escape
(pipe_escape) @string.escape
(marker) @keyword
(inline_kind) @keyword
(verbatim_kind) @keyword

; Parsed inline delimiters.
(inline_element
  "[" @punctuation.bracket
  "]" @punctuation.bracket)

(member_separator) @punctuation.delimiter
(argument_separator) @punctuation.delimiter

(verbatim_open) @punctuation.delimiter
(raw_tail_open) @punctuation.delimiter

; Raw payloads are syntax nodes because they change the lexical mode.
((inline_verbatim (raw_text) @markup.raw)
  (#set! priority 90))
((verbatim_argument (raw_text) @markup.raw)
  (#set! priority 90))
(verbatim_block (raw_text) @markup.raw.block)
(raw_tail (raw_text) @markup.raw.block)

; Recovery nodes represent incomplete editor input, not valid strict syntax.
[
  (incomplete_inline_element)
] @error
