local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local root = vim.fn.tempname()
vim.fn.mkdir(root, 'p')
local current = root .. '/current.plumb'
vim.fn.writefile({ 'Current note' }, current)
local bufnr = vim.fn.bufadd(current)
vim.fn.bufload(bufnr)
vim.bo[bufnr].filetype = 'plumb'
vim.api.nvim_set_current_buf(bufnr)

local original_get_clients = vim.lsp.get_clients
local original_system = vim.system
local commands = {}
vim.lsp.get_clients = function(opts)
  assert(opts.bufnr == bufnr)
  return { { config = { root_dir = root } } }
end
vim.system = function(command, opts, callback)
  assert(opts.detach == true)
  table.insert(commands, command)
  callback({ code = 0, stdout = '', stderr = '' })
  return { pid = 1 }
end

local graph = require('plumb.graph')
graph.setup({ command = repo .. '/target/debug/plumb' })
vim.cmd('PlumbGraph')
assert(vim.deep_equal(commands[1], {
  repo .. '/target/debug/plumb', 'graph', '--root', root, '--current', current,
}))
vim.cmd('PlumbGraph!')
assert(vim.deep_equal(commands[2], {
  repo .. '/target/debug/plumb', 'graph', '--root', root,
}))

vim.lsp.get_clients = original_get_clients
vim.system = original_system
vim.fn.delete(root, 'rf')
