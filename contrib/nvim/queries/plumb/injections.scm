(verbatim_block
  (attributes
    (attribute_pair
      key: (attribute_name) @_language_key
      value: (attribute_value) @injection.language))
  body: (raw_text) @injection.content
  (#eq? @_language_key "language")
  (#set! injection.combined))

(verbatim_block
  (attributes
    (attribute_class
      (attribute_name) @_math_class))
  body: (raw_text) @injection.content
  (#eq? @_math_class "$" )
  (#set! injection.language "latex")
  (#set! injection.combined))
