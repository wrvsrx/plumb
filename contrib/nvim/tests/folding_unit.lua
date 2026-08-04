local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local folding = require('plumb.folding')
local original_foldtext = vim.lsp.foldtext

local function render(text)
  vim.lsp.foldtext = function()
    return text
  end
  return folding.foldtext()
end

assert(vim.deep_equal(render('  +   Ship it'), {
  { '  +   Ship it', '@lsp.typemod.task.completed.plumb' },
}), 'highlight a completed task fold')
assert(vim.deep_equal(render('x   Superseded'), {
  { 'x   Superseded', '@lsp.typemod.task.canceled.plumb' },
}), 'highlight a canceled task fold')
assert(vim.deep_equal(render('+x  Conflicted'), {
  { '+x  Conflicted', '@lsp.typemod.task.completed.plumb' },
}), 'highlight a conflicted task fold')

for _, text in ipairs({
  '-   Ship it',
  '~   Ship it',
  '!   Ship it',
  'METADATA  Project',
  '2026-08-04T09:30  Meeting',
  '`note Details',
}) do
  assert(render(text) == text, 'preserve unhighlighted fold text: ' .. text)
end

local chunks = { { 'already styled', 'Title' } }
assert(render(chunks) == chunks, 'preserve non-string native fold text')
vim.lsp.foldtext = original_foldtext
