local repo = vim.fn.getcwd()
local runtime = repo .. '/contrib/nvim'
vim.opt.runtimepath:prepend(runtime)
dofile(runtime .. '/ftdetect/plumb.lua')
assert(vim.filetype.match({ filename = 'note.plumb' }) == 'plumb')

local plumb = require('plumb')
local opts = {
  command = repo .. '/target/debug/plumb',
  codelens = { enabled = true, picker = 'quickfix' },
  search = { enabled = false },
}
vim.api.nvim_set_hl(0, 'PlumbTaskFoldWaiting', { fg = 0x123456 })
plumb.setup(opts)
assert(
  vim.api.nvim_get_hl(0, { name = 'PlumbTaskFoldWaiting', link = false }).fg == 0x123456,
  'production setup preserves an existing highlight override'
)
assert(package.loaded['plumb.codelens'] == nil, 'defer the CodeLens implementation')
assert(package.loaded['plumb.search'] == nil, 'defer the search implementation')
local first = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
plumb.setup(opts)
local second = #vim.api.nvim_get_autocmds({ group = 'PlumbNvim' })
assert(first == second, 'setup leaked autocmds')
for _, name in ipairs({ 'PlumbFormat', 'PlumbRename', 'PlumbCodeAction', 'PlumbTaskComplete', 'PlumbTaskCancel' }) do
  assert(vim.fn.exists(':' .. name) == 0, name .. ' should remain a native LSP action')
end
assert(vim.fn.exists(':PlumbNotes') == 0)
assert(vim.fn.exists(':PlumbReferences') == 0)
assert(type(vim.lsp.commands['plumb.showReferences']) == 'function')
assert(vim.fn.exists(':PlumbWeb') == 0)
vim.lsp.commands['plumb.showReferences']({ arguments = { '', {}, {} } }, {})
assert(package.loaded['plumb.codelens'] ~= nil, 'load CodeLens on first execution')

plumb.setup({
  lsp = { enabled = false },
  codelens = { enabled = false },
  search = { enabled = false },
})
assert(vim.lsp.commands['plumb.showReferences'] == nil, 'remove a disabled CodeLens handler')
