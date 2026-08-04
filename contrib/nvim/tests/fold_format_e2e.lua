local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')
vim.cmd.filetype('on')
dofile(repo .. '/contrib/nvim/ftdetect/plumb.lua')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/.plumb', 'p')
local path = root .. '/fold-format.plumb'
vim.fn.writefile({
  '`-{.task #first} First',
  '',
  '   `note Detail',
  '',
  '`-{.task #second} Second',
  '',
  '  `note Detail',
}, path)

require('plumb').setup({
  command = repo .. '/target/debug/plumb',
  codelens = { enabled = true },
  search = { enabled = false },
})
vim.cmd.edit(vim.fn.fnameescape(path))
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
end), 'attach plumb LSP')
local client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])

vim.wo.foldmethod = 'expr'
vim.wo.foldexpr = "v:lua.require'plumb'.foldexpr()"
vim.wo.foldtext = "v:lua.require'plumb'.foldtext()"
vim.wo.foldminlines = 0
vim.wo.foldlevel = 99
vim.cmd('normal! zx')
assert(vim.wait(5000, function()
  return vim.fn.foldlevel(1) > 0 and vim.fn.foldlevel(5) > 0
end), 'receive folding ranges')

vim.cmd('normal! zM')
vim.api.nvim_win_set_cursor(0, { 5, 0 })
vim.cmd('normal! zo')
assert(vim.fn.foldclosed(1) == 1, 'first task should remain closed')
assert(vim.fn.foldclosed(5) == -1, 'second task should be manually open')

vim.lsp.buf.format({ name = 'plumb', async = false, timeout_ms = 5000 })
assert(vim.wait(5000, function()
  return vim.api.nvim_buf_get_lines(0, 6, 7, false)[1] == '   `note Detail'
end), 'format the manually opened task')
assert(vim.wait(5000, function()
  return vim.fn.foldlevel(1) > 0 and vim.fn.foldlevel(5) > 0
end), 'refresh folding ranges after format')
assert(vim.fn.foldclosed(1) == 1, 'format should preserve the closed first task')
assert(vim.fn.foldclosed(5) == -1, 'format should preserve the manually opened second task')

client:stop(true)
assert(vim.wait(5000, function()
  return vim.lsp.get_client_by_id(client.id) == nil
end), 'shutdown plumb LSP')
vim.fn.delete(root, 'rf')
