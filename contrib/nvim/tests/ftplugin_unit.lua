local repo = vim.fn.getcwd()
local runtime = repo .. '/contrib/nvim'
vim.opt.runtimepath:prepend(runtime)
vim.cmd.filetype('plugin on')
dofile(runtime .. '/ftdetect/plumb.lua')

vim.o.expandtab = false
vim.o.shiftwidth = 4
vim.o.softtabstop = 3
vim.o.tabstop = 7

vim.cmd.edit(vim.fn.fnameescape(vim.fn.tempname() .. '.plumb'))
assert(vim.bo.filetype == 'plumb')
assert(vim.bo.expandtab == true, 'enable space indentation')
assert(vim.bo.shiftwidth == 1, 'use one structural space per indentation level')
assert(vim.bo.softtabstop == -1, 'follow shiftwidth while editing')
assert(vim.bo.tabstop == 7, 'preserve the user tab display width for verbatim payloads')

vim.cmd('set filetype=text')
assert(vim.bo.expandtab == false, 'restore expandtab after leaving plumb')
assert(vim.bo.shiftwidth == 4, 'restore shiftwidth after leaving plumb')
assert(vim.bo.softtabstop == 3, 'restore softtabstop after leaving plumb')
assert(vim.bo.tabstop == 7, 'leave tabstop unchanged after leaving plumb')
