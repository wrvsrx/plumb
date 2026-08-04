local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')
vim.cmd.filetype('on')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/.plumb', 'p')
local path = root .. '/single-line-fold.plumb'
vim.fn.writefile({
  '`-{.task} Ready task',
  '`-{.task done="2026-08-04T09:00:00+08:00"} Finished task',
  '`-{.event date=2026-08-04 timezone="+08:00" when="10:00"} Standup',
  'Plain text',
  '`-{.task} EOF task',
}, path)

require('plumb').setup({
  command = repo .. '/target/debug/plumb',
  search = { enabled = false },
})
vim.cmd.edit(vim.fn.fnameescape(path))
assert(vim.wait(5000, function()
  return #vim.lsp.get_clients({ bufnr = 0, name = 'plumb' }) == 1
end), 'attach plumb LSP')

vim.wo.foldmethod = 'expr'
vim.wo.foldexpr = "v:lua.require'plumb'.foldexpr()"
vim.wo.foldtext = "v:lua.require'plumb'.foldtext()"
vim.wo.foldminlines = 0
vim.wo.foldlevel = 99
vim.cmd('normal! zx')
assert(vim.wait(5000, function()
  return vim.fn.foldlevel(1) == 1
    and vim.fn.foldlevel(2) == 1
    and vim.fn.foldlevel(3) == 1
    and vim.fn.foldlevel(4) == 0
    and vim.fn.foldlevel(5) == 1
end), 'receive single-line folding ranges')

vim.cmd('normal! zM')
for _, line in ipairs({ 1, 2, 3, 5 }) do
  assert(vim.fn.foldclosed(line) == line, 'close single-line fold at line ' .. line)
  assert(vim.fn.foldclosedend(line) == line, 'end single-line fold at line ' .. line)
end
assert(vim.fn.foldtextresult(1) == 'READY  Ready task', 'render ready task fold text')
assert(vim.fn.foldtextresult(2) == 'DONE  Finished task', 'render done task fold text')
assert(vim.fn.foldtextresult(3) == '2026-08-04T10:00  Standup', 'render event fold text')
assert(vim.fn.foldtextresult(5) == 'READY  EOF task', 'render EOF task fold text')

local client = assert(vim.lsp.get_clients({ bufnr = 0, name = 'plumb' })[1])
client:stop(true)
assert(vim.wait(5000, function()
  return vim.lsp.get_client_by_id(client.id) == nil
end), 'shutdown plumb LSP')
vim.fn.delete(root, 'rf')
