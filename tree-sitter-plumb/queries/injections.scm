(marked_block
  marker: (marker) @injection.language
  raw: (raw_tail
    body: (raw_text)+ @injection.content)
  (#set! injection.combined))

(marked_block
  marker: (marker) @_math_kind
  raw: (raw_tail
    body: (raw_text)+ @injection.content)
  (#eq? @_math_kind "$" )
  (#set! injection.language "latex")
  (#set! injection.combined))
