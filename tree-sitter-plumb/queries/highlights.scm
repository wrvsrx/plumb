; Core block and inline envelopes. Marker and kind meanings remain opaque.
(introducer) @punctuation.special
(introducer_escape) @string.escape
(marker) @keyword
(code_marker) @punctuation.delimiter
(inline_kind) @keyword

; Parsed inline delimiters.
(inline_element
  "[" @punctuation.bracket
  "]" @punctuation.bracket)

; Attributes and their local punctuation.
(attributes
  "{" @punctuation.bracket
  "}" @punctuation.bracket)

(incomplete_attributes
  "{" @punctuation.bracket)

(attribute_id
  "#" @punctuation.special
  (attribute_name) @attribute)

(attribute_class
  "." @punctuation.special
  (attribute_name) @attribute)

(attribute_pair
  key: (attribute_name) @property
  "=" @operator
  value: (attribute_value) @string)

; Raw payloads are syntax nodes because they change the lexical mode.
((inline_verbatim) @markup.raw
  (#set! priority 90))
(code_block (raw_text) @markup.raw.block)

; Recovery nodes represent incomplete editor input, not valid strict syntax.
[
  (incomplete_inline_element)
  (incomplete_attributes)
] @error
