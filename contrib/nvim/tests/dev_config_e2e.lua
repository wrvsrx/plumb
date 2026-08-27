local repo = vim.fn.getcwd()

vim.api.nvim_set_hl(0, 'Comment', { fg = 0x667788, italic = true })
vim.api.nvim_set_hl(0, 'DiagnosticInfo', { fg = 0xff0000 })
vim.api.nvim_set_hl(0, 'DiagnosticHint', { fg = 0x00ff00 })
vim.api.nvim_set_hl(0, 'DiagnosticDeprecated', { fg = 0x0000ff, strikethrough = true })
vim.api.nvim_set_hl(0, 'DiagnosticWarn', { fg = 0xffff00 })

-- Simulate defaults left behind when the user's init loads a packaged plugin
-- before the repository-local exrc runs.
vim.api.nvim_set_hl(0, '@lsp.typemod.task.canceled.plumb', { link = 'DiagnosticDeprecated' })
vim.api.nvim_set_hl(0, 'PlumbTaskFoldWaiting', { link = 'Error' })
vim.api.nvim_set_hl(0, 'PlumbTaskFoldCanceled', { link = 'DiagnosticDeprecated' })

dofile(repo .. '/dev/nvim.lua')

assert(
  vim.api.nvim_get_hl(0, { name = '@lsp.typemod.task.canceled.plumb', link = true }).link
    == 'Comment',
  'replace the packaged canceled semantic-token default'
)
assert(
  vim.api.nvim_get_hl(0, { name = 'PlumbTaskFoldWaiting', link = true }).link
    == 'DiagnosticInfo',
  'replace the packaged waiting fold default'
)
local canceled = vim.api.nvim_get_hl(0, { name = 'PlumbTaskFoldCanceled', link = false })
assert(canceled.fg == 0x667788, 'rebuild canceled fold with the current Comment foreground')
assert(canceled.italic == true, 'rebuild canceled fold with the current Comment style')
assert(canceled.strikethrough == true, 'preserve the canceled fold strikethrough')
assert(canceled.fg ~= 0x0000ff, 'do not retain the packaged diagnostic color')
