local repo = vim.fn.getcwd()
local runtime = repo .. '/contrib/nvim'
vim.opt.runtimepath:prepend(runtime)
dofile(runtime .. '/plugin/plumb.lua')
assert(vim.filetype.match({ filename = 'note.plumb' }) == 'plumb')

local plumb = require('plumb')
local opts = {
  command = repo .. '/target/debug/plumb',
  codelens = { enabled = false },
  search = { enabled = false },
}
plumb.setup(opts)
local first = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
plumb.setup(opts)
local second = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
assert(first == second, 'setup leaked autocmds')
for _, name in ipairs({ 'PlumbFormat', 'PlumbRename', 'PlumbCodeAction', 'PlumbTaskComplete', 'PlumbTaskCancel' }) do
  assert(vim.fn.exists(':' .. name) == 0, name .. ' should remain a native LSP action')
end
assert(vim.fn.exists(':PlumbNotes') == 0)
assert(vim.fn.exists(':PlumbReferences') == 0)
assert(vim.fn.exists(':PlumbWeb') == 0)

plumb.setup({
  lsp = { enabled = false },
  codelens = { enabled = false },
  search = { enabled = false },
})
