local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local folding = require('plumb.folding')
local levels, texts = folding._evaluate({
  { startLine = 0, endLine = 0, collapsedText = 'READY  One' },
  { startLine = 1, endLine = 3, collapsedText = 'DONE  Parent' },
  { startLine = 2, endLine = 2, collapsedText = 'CANCELED  Child' },
}, 4)

assert(vim.deep_equal(levels, { [0] = 1, [2] = 1 }), 'retain only single-line ranges')
assert(vim.deep_equal(texts, {
  [0] = 'READY  One',
  [2] = 'CANCELED  Child',
}), 'cache collapsed text by start row')

levels, texts = folding._evaluate({
  { startLine = -1, endLine = 0, collapsedText = 'invalid' },
  { startLine = 2, endLine = 1, collapsedText = 'invalid' },
  { startLine = 0, endLine = 4, collapsedText = 'invalid' },
}, 4)
assert(vim.tbl_isempty(levels) and vim.tbl_isempty(texts), 'ignore invalid ranges')

assert(vim.deep_equal(folding._style_text('  DONE  Ship it'), {
  { '  DONE  Ship it', '@lsp.typemod.task.completed.plumb' },
}), 'highlight a completed task fold')
assert(vim.deep_equal(folding._style_text('CANCELED  Superseded'), {
  { 'CANCELED  Superseded', '@lsp.typemod.task.canceled.plumb' },
}), 'highlight a canceled task fold')
assert(folding._style_text('READY  Ship it') == 'READY  Ship it', 'preserve ready fold text')
