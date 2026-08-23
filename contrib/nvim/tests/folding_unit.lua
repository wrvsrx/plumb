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

assert(vim.deep_equal(render('`task [o]  Ship it'), {
  { '`task [o]  Ship it', 'PlumbTaskFoldDone' },
}), 'highlight a completed task fold')
assert(vim.deep_equal(render('`task [x]  Superseded'), {
  { '`task [x]  Superseded', 'PlumbTaskFoldCanceled' },
}), 'highlight a canceled task fold')
assert(vim.deep_equal(render('`task [ox] Conflicted'), {
  { '`task [ox] Conflicted', 'PlumbTaskFoldConflicted' },
}), 'highlight a conflicted task fold')
assert(vim.deep_equal(render('`task [~]  Waiting'), {
  { '`task [~]  Waiting', 'PlumbTaskFoldWaiting' },
}), 'highlight a waiting task fold')
assert(vim.deep_equal(render('`task [=]  Blocked'), {
  { '`task [=]  Blocked', 'PlumbTaskFoldBlocked' },
}), 'highlight a blocked task fold')

for _, text in ipairs({
  '`task [ ]  Ship it',
  'METADATA  Project',
  '2026-08-04T09:30  Meeting',
  '`note Details',
}) do
  assert(render(text) == text, 'preserve unhighlighted fold text: ' .. text)
end

local chunks = { { 'already styled', 'Title' } }
assert(render(chunks) == chunks, 'preserve non-string native fold text')
vim.lsp.foldtext = original_foldtext
