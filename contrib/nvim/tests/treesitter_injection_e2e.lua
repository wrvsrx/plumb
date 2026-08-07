local repo = vim.fn.getcwd()
local grammar = repo .. '/tree-sitter-plumb'
local python_package = assert(
  os.getenv('TREE_SITTER_PYTHON_PACKAGE'),
  'set TREE_SITTER_PYTHON_PACKAGE to the tree-sitter-python package root'
)

vim.treesitter.language.add('plumb', { path = grammar .. '/build/plumb.so' })
vim.treesitter.language.add('python', { path = python_package .. '/parser' })

local function read_query(path)
  return table.concat(vim.fn.readfile(path), '\n')
end

vim.treesitter.query.set('plumb', 'injections', read_query(grammar .. '/queries/injections.scm'))
vim.treesitter.query.set('python', 'highlights', read_query(python_package .. '/queries/highlights.scm'))

local function parse_injection(lines, language)
  local bufnr = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  local parser = vim.treesitter.get_parser(bufnr, 'plumb')
  parser:parse(true)
  local child = assert(parser:children()[language], 'missing ' .. language .. ' injection')
  local tree = assert(child:trees()[1], 'missing ' .. language .. ' injection tree')
  return bufnr, parser:trees()[1]:root(), tree:root()
end

local plumb_buf, _, plumb_root = parse_injection({
  '`plumb"',
  ' `- xixi',
  '  {',
  '   `: recur P1D',
  '  }',
  '',
  ' `# some',
}, 'plumb')
assert(not plumb_root:has_error(), 'recursive plumb injection must parse without errors')
local plumb_tree = plumb_root:sexpr()
assert(plumb_tree:find('attached_block_group', 1, true), 'recursive injection must include the group')
assert(plumb_tree:find('blank_line', 1, true), 'recursive injection must include all raw lines')
vim.api.nvim_buf_delete(plumb_buf, { force = true })

local python_buf, outer_root, python_root = parse_injection({
  '`python""',
  '  def greet(name):',
  '      if name:',
  '          return f"hello {name}"',
}, 'python')
assert(not python_root:has_error(), 'indentation-sensitive Python injection must parse without errors')

local raw_query = vim.treesitter.query.parse('plumb', '(raw_text) @raw')
local raw_lines = {}
for _, node in raw_query:iter_captures(outer_root, python_buf, 0, -1) do
  raw_lines[#raw_lines + 1] = vim.treesitter.get_node_text(node, python_buf)
end
assert(
  raw_lines[1] == 'def greet(name):\n',
  'raw range must exclude the two-space margin: ' .. vim.inspect(raw_lines)
)
assert(
  raw_lines[2] == '    if name:\n',
  'raw range must preserve Python indentation: ' .. vim.inspect(raw_lines)
)
assert(
  raw_lines[3] == '        return f"hello {name}"\n',
  'nested Python indentation must be unchanged: ' .. vim.inspect(raw_lines)
)

local python_query = vim.treesitter.query.get('python', 'highlights')
local captures = {}
for id, node in python_query:iter_captures(python_root, python_buf, 0, -1) do
  local name = python_query.captures[id]
  local value = vim.treesitter.get_node_text(node, python_buf)
  captures[name .. '\0' .. value] = true
end
assert(captures['function\0greet'], 'Python function name must receive its injected highlight')
assert(captures['keyword\0if'], 'Python if must receive its injected highlight')
assert(captures['keyword\0return'], 'Python return must receive its injected highlight')
vim.api.nvim_buf_delete(python_buf, { force = true })
