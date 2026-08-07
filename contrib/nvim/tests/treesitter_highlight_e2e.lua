local repo = vim.fn.getcwd()
local grammar = repo .. '/tree-sitter-plumb'
local parser_path = grammar .. '/build/plumb.so'

assert(vim.uv.fs_stat(parser_path), 'build tree-sitter-plumb before running this test')
vim.treesitter.language.add('plumb', { path = parser_path })

local bufnr = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, {
  '`- xixi',
  ' {',
  '  `: test 2026-08-06T00:00:00+08:00',
  '  `: recur P1D',
  '  `: prev #take-diary-2026-08-05',
  ' }',
  '',
  '`# some',
  '',
  '`->"something.plumb"',
})

local parser = vim.treesitter.get_parser(bufnr, 'plumb')
local root = parser:parse()[1]:root()
assert(not root:has_error(), 'valid next-line attached group must not produce an error node')

local crlf_source = table.concat({
  '`- crlf',
  ' {',
  '  `: value nested',
  ' }',
  '',
  '',
  '`# top',
  '',
}, '\r\n')
local crlf_root = vim.treesitter.get_string_parser(crlf_source, 'plumb'):parse()[1]:root()
assert(
  not crlf_root:has_error(),
  'next-line attached group must dedent across CRLF blank lines: ' .. crlf_root:sexpr()
)

local query_source = table.concat(vim.fn.readfile(grammar .. '/queries/highlights.scm'), '\n')
local query = vim.treesitter.query.parse('plumb', query_source)
local captures = {}
for id, node in query:iter_captures(root, bufnr, 0, -1) do
  local name = query.captures[id]
  local text = vim.treesitter.get_node_text(node, bufnr)
  captures[name .. '\0' .. text] = true
end

assert(captures['label\0->'], 'line-start inline verbatim kind must receive the label capture')
assert(
  captures['markup.raw\0"something.plumb"'],
  'line-start inline verbatim payload must receive a separate raw capture'
)

vim.api.nvim_buf_delete(bufnr, { force = true })
