(verbatim_block
  kind: (verbatim_kind) @injection.language
  body: (raw_text) @injection.content
  (#set! injection.combined))

(verbatim_block
  kind: (verbatim_kind) @_math_kind
  body: (raw_text) @injection.content
  (#eq? @_math_kind "$" )
  (#set! injection.language "latex")
  (#set! injection.combined))
