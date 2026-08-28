local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')
vim.cmd.filetype('on')
dofile(repo .. '/contrib/nvim/ftdetect/plumb.lua')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/.plumb', 'p')
local path = root .. '/fold-join.plumb'
vim.fn.writefile({
  '`task aaa bbb ccc ddd eee fff',
  ' continuation ggg hhh iii jjj',
  ' continuation kkk lll mmm nnn',
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

local function wait_for_folding_response()
  local done = false
  client:request('textDocument/foldingRange', {
    textDocument = vim.lsp.util.make_text_document_params(0),
  }, function(err)
    assert(not err, vim.inspect(err))
    done = true
  end, 0)
  assert(vim.wait(5000, function()
    return done
  end), 'receive folding range response')
  vim.wait(500)
end

vim.wo.foldmethod = 'expr'
vim.wo.foldexpr = 'v:lua.vim.lsp.foldexpr()'
vim.wo.foldtext = "v:lua.require'plumb'.foldtext()"
vim.wo.foldminlines = 0
vim.wo.foldlevel = 99
vim.cmd('normal! zx')
assert(vim.wait(5000, function()
  return vim.fn.foldlevel(1) > 0
end), 'receive folding range')

vim.cmd('normal! zMzo')
assert(vim.fn.foldclosed(1) == -1, 'task should be manually open')

vim.cmd('normal! J')
assert(vim.wait(5000, function()
  return vim.api.nvim_buf_line_count(0) == 2
end), 'join the first two task lines')
wait_for_folding_response()
assert(vim.wait(5000, function()
  return vim.fn.foldlevel(1) > 0
end), 'refresh the two-line folding range')
assert(vim.fn.foldclosed(1) == -1, 'first join should preserve the manually opened task')

vim.cmd('normal! J')
assert(vim.wait(5000, function()
  return vim.api.nvim_buf_line_count(0) == 1
end), 'join the complete task onto one line')
wait_for_folding_response()
assert(vim.fn.foldclosed(1) == -1, 'second join should preserve the manually opened task')

client:stop(true)
assert(vim.wait(5000, function()
  return vim.lsp.get_client_by_id(client.id) == nil
end), 'shutdown plumb LSP')
vim.fn.delete(root, 'rf')
