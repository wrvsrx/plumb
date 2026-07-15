(introducer) @punctuation.special
(introducer_escape) @string.escape
(marker) @type
(code_marker) @punctuation.special
(inline_kind) @function.macro

(attribute_tag) @tag
(attribute_id) @attribute
(attribute_class) @attribute
(attribute_pair key: (attribute_name) @property)
(attribute_pair value: (attribute_value) @string)

(inline_verbatim) @string
(code_block (raw_text) @string)

[
  (incomplete_inline_element)
  (incomplete_attributes)
] @error
