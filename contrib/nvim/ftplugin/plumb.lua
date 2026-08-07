vim.bo.expandtab = true
vim.bo.shiftwidth = 1
vim.bo.softtabstop = -1

vim.b.undo_ftplugin = (vim.b.undo_ftplugin or '')
  .. '\n setlocal expandtab< shiftwidth< softtabstop<'
