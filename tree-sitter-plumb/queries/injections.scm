((code_block
  (attributes
    (attribute_pair
      key: (attribute_name) @_language_key
      value: (attribute_value) @injection.language))
  body: (raw_text) @injection.content)
 (#eq? @_language_key "language"))
