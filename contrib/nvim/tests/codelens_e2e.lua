local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local path = vim.fn.tempname() .. '.plumb'
vim.fn.writefile({ 'reference target' }, path)
local uri = vim.uri_from_fname(path)
local captured
package.loaded.snacks = {
  picker = {
    pick = function(opts)
      captured = opts
    end,
  },
}

local get_client_by_id = vim.lsp.get_client_by_id
vim.lsp.get_client_by_id = function(client_id)
  assert(client_id == 7)
  return { offset_encoding = 'utf-16' }
end

require('plumb.codelens').setup({ picker = 'snacks' })
vim.lsp.commands['plumb.showReferences']({
  title = '1 file reference',
  arguments = {
    uri,
    { line = 0, character = 0 },
    {
      {
        uri = uri,
        range = {
          start = { line = 0, character = 0 },
          ['end'] = { line = 0, character = 9 },
        },
      },
    },
  },
}, { client_id = 7 })

assert(captured, 'Snacks picker was not called')
assert(captured.title == '1 file reference')
assert(captured.format == 'file')
assert(captured.preview == 'file')
assert(captured.confirm == 'jump')
assert(#captured.items == 1)
assert(captured.items[1].file == path)
assert(captured.items[1].pos[1] == 1)
assert(captured.items[1].pos[2] == 0)
assert(captured.items[1].line == 'reference target')

vim.lsp.get_client_by_id = get_client_by_id
vim.fn.delete(path)
