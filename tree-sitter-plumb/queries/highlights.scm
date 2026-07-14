(marker) @punctuation.special
(code_marker) @punctuation.special
(inline_kind) @function.macro
(introducer) @punctuation.special
(introducer_escape) @string.escape

(attribute_tag) @tag
(attribute_id) @attribute
(attribute_class) @attribute
(attribute_pair key: (attribute_name) @property)
(attribute_pair value: (attribute_value) @string)

(inline_verbatim (raw_text) @string)
(code_block (code_line (raw_text) @string))
