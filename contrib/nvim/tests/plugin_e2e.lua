local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')
vim.cmd.filetype('on')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/.plumb', 'p')
local path = root .. '/plugin.plumb'
vim.fn.writefile({
  '`meta',
  '   `: title',
  '',
  '      Plugin E2E',
  '',
  '`-{.task #work} Do work',
  '  `note Task detail',
}, path)

require('plumb').setup({
  command = repo .. '/target/debug/plumb',
  codelens = { enabled = true, picker = 'quickfix' },
  search = { enabled = true },
  web = { enabled = false },
})
vim.cmd.edit(vim.fn.fnameescape(path))
assert(vim.bo.filetype == 'plumb')
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
end), 'attach plumb LSP through native root markers')
local client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])
assert(vim.fs.normalize(client.config.root_dir) == vim.fs.normalize(root))
assert(
  vim.api.nvim_get_hl(0, { name = '@lsp.typemod.task.completed.plumb' }).link == 'Comment',
  'dim completed task tokens'
)
assert(
  vim.api.nvim_get_hl(0, { name = '@lsp.typemod.task.canceled.plumb' }).link == 'Comment',
  'dim canceled task tokens'
)
assert(vim.wo.foldmethod == 'expr', 'configure LSP folding method')
assert(vim.wo.foldexpr == 'v:lua.vim.lsp.foldexpr()', 'configure LSP folding expression')
assert(vim.wo.foldtext == 'v:lua.vim.lsp.foldtext()', 'configure LSP folding text')
assert(vim.wo.foldlevel == 99, 'keep folds initially open')

local completion_path = root .. '/completion.plumb'
vim.fn.writefile({ '`' }, completion_path)
vim.cmd.edit(vim.fn.fnameescape(completion_path))
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
end), 'attach completion buffer')
client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])
local completion = client:request_sync('textDocument/completion', {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 0, character = 1 },
}, 5000, 0)
assert(completion and not completion.err)
local items = completion.result.items or completion.result
assert(vim.iter(items):any(function(item) return item.label == 'Task' end), 'Task completion missing')

vim.cmd.edit(vim.fn.fnameescape(path))
vim.cmd.PlumbFormat()
assert(vim.api.nvim_buf_get_lines(0, 0, 4, false)[2] == ' `: title', 'format command did not apply')
vim.api.nvim_win_set_cursor(0, { 6, 0 })
assert(vim.wait(5000, function()
  return #vim.lsp.codelens.get({ bufnr = 0, client_id = client.id }) > 0
end), 'receive plumb CodeLens')

vim.cmd.PlumbTaskComplete()
assert(vim.wait(5000, function()
  return table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '\n'):match('done="') ~= nil
end), 'complete task through documented command')
local folds
assert(vim.wait(5000, function()
  folds = client:request_sync('textDocument/foldingRange', {
    textDocument = { uri = vim.uri_from_bufnr(0) },
  }, 1000, 0)
  return folds and not folds.err and vim.iter(folds.result):any(function(range)
    return range.collapsedText == 'DONE  Do work'
  end)
end), 'label completed task fold: ' .. vim.inspect(folds))

vim.api.nvim_buf_set_lines(0, -1, -1, false, { '`node{key=a key=b} Invalid' })
assert(vim.wait(5000, function()
  return #vim.diagnostic.get(0) > 0
end), 'receive strict diagnostics')

local second_root = vim.fn.tempname()
vim.fn.mkdir(second_root .. '/.plumb', 'p')
local second_path = second_root .. '/second.plumb'
vim.fn.writefile({ 'Second workspace' }, second_path)
vim.cmd.edit(vim.fn.fnameescape(second_path))
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
    and #vim.lsp.get_clients({ name = 'plumb' }) == 2
end), 'start one client per workspace root')
local second_client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])
assert(vim.fs.normalize(second_client.config.root_dir) == vim.fs.normalize(second_root))

second_client:stop(true)
client:stop(true)
assert(vim.wait(5000, function()
  return vim.lsp.get_client_by_id(client.id) == nil
    and vim.lsp.get_client_by_id(second_client.id) == nil
end), 'shutdown all plumb LSP clients')
vim.fn.delete(root, 'rf')
vim.fn.delete(second_root, 'rf')
