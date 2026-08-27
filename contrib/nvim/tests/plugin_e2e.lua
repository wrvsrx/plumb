local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')
vim.cmd.filetype('on')
dofile(repo .. '/contrib/nvim/ftdetect/plumb.lua')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/.plumb', 'p')
local path = root .. '/plugin.plumb'
vim.fn.writefile({
  '`= title Plugin E2E',
  '',
  '`task Do work',
  ' `@ work',
  '',
  ' `note Task detail',
}, path)

vim.api.nvim_set_hl(0, 'Comment', { fg = 0x667788, italic = true })
vim.api.nvim_set_hl(0, 'DiagnosticInfo', { fg = 0xff0000, underline = true })
vim.api.nvim_set_hl(0, 'DiagnosticHint', { fg = 0x00ff00, undercurl = true })
vim.api.nvim_set_hl(0, 'DiagnosticDeprecated', { fg = 0x0000ff, strikethrough = true })
vim.api.nvim_set_hl(0, 'DiagnosticWarn', { fg = 0xffff00, bold = true })

require('plumb').setup({
  command = repo .. '/target/debug/plumb',
  codelens = { enabled = true, picker = 'quickfix' },
  search = { enabled = true },
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
local task_fold_decorations = {
  PlumbTaskFoldWaiting = 'underline',
  PlumbTaskFoldBlocked = 'undercurl',
  PlumbTaskFoldDone = nil,
  PlumbTaskFoldCanceled = 'strikethrough',
  PlumbTaskFoldConflicted = 'bold',
}
for _, group in ipairs({
  'PlumbTaskFoldWaiting',
  'PlumbTaskFoldBlocked',
  'PlumbTaskFoldDone',
  'PlumbTaskFoldCanceled',
  'PlumbTaskFoldConflicted',
}) do
  local highlight = vim.api.nvim_get_hl(0, { name = group, link = false })
  assert(highlight.fg == 0x667788, group .. ' uses the Comment foreground')
  assert(highlight.italic == true, group .. ' preserves the Comment base style')
  local decoration = task_fold_decorations[group]
  if decoration then
    assert(highlight[decoration] == true, group .. ' preserves its diagnostic decoration')
  end
end

local completion_path = root .. '/completion.plumb'
vim.fn.writefile({ '`t' }, completion_path)
vim.cmd.edit(vim.fn.fnameescape(completion_path))
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
end), 'attach completion buffer')
client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])
local completion = client:request_sync('textDocument/completion', {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 0, character = 2 },
}, 5000, 0)
assert(completion and not completion.err)
local items = completion.result.items or completion.result
assert(vim.iter(items):any(function(item) return item.label == 'Task' end), 'Task completion missing')

vim.cmd.edit(vim.fn.fnameescape(path))
vim.api.nvim_win_set_cursor(0, { 6, 0 })
assert(vim.wait(5000, function()
  return #vim.lsp.codelens.get({ bufnr = 0, client_id = client.id }) > 0
end), 'receive plumb CodeLens')

vim.api.nvim_buf_set_lines(0, -1, -1, false, { 'Unclosed `kind[inline element' })
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
