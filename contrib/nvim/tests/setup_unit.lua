local repo = vim.fn.getcwd()
local runtime = repo .. '/contrib/nvim'
vim.opt.runtimepath:prepend(runtime)
dofile(runtime .. '/plugin/plumb.lua')
assert(vim.filetype.match({ filename = 'note.plumb' }) == 'plumb')

local plumb = require('plumb')
local opts = {
  command = repo .. '/target/debug/plumb',
  treesitter = { enabled = false },
  codelens = { enabled = false },
  search = { enabled = false },
  web = { enabled = false },
}
plumb.setup(opts)
local first = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
plumb.setup(opts)
local second = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
assert(first == second, 'setup leaked autocmds')
for _, name in ipairs({
  'PlumbFormat',
  'PlumbRename',
  'PlumbCodeAction',
  'PlumbTaskComplete',
  'PlumbTaskCancel',
}) do
  assert(vim.fn.exists(':' .. name) == 2, name .. ' was not registered')
end
assert(vim.fn.exists(':PlumbNotes') == 0)
assert(vim.fn.exists(':PlumbReferences') == 0)
assert(vim.fn.exists(':PlumbWeb') == 0)

plumb.setup({
  lsp = { enabled = false },
  treesitter = { enabled = false },
  codelens = { enabled = false },
  search = { enabled = false },
  web = { enabled = false },
})
assert(vim.fn.exists(':PlumbFormat') == 0, 'disabled LSP commands were retained')

for _, name in ipairs({ 'folds.scm', 'highlights.scm', 'indents.scm', 'injections.scm', 'textobjects.scm' }) do
  local source = vim.fn.readfile(repo .. '/tree-sitter-plumb/queries/' .. name)
  local bundled = vim.fn.readfile(runtime .. '/queries/plumb/' .. name)
  assert(vim.deep_equal(source, bundled), name .. ' drifted from tree-sitter source')
end
