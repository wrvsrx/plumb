local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local search = require('plumb.search')
local root = vim.fn.tempname()
vim.fn.mkdir(root, 'p')
local current = root .. '/current.plumb'
local target = root .. '/target.plumb'
vim.fn.writefile({ '`->[' }, current)
vim.fn.writefile({ '`meta', ' `: title', '', '    Target note', '' }, target)

local bufnr = vim.fn.bufadd(current)
vim.fn.bufload(bufnr)
local client_id = vim.lsp.start({
  name = 'plumb-search-e2e',
  cmd = { repo .. '/target/debug/plumb', 'lsp' },
  root_dir = root,
}, { bufnr = bufnr })
assert(client_id, 'start plumb LSP')
assert(vim.wait(5000, function()
  local client = vim.lsp.get_client_by_id(client_id)
  return client and client.initialized
end), 'initialize plumb LSP')

local capability, capability_error = search.capabilities(bufnr)
assert(capability, capability_error)
assert(capability.schema_version == 1)

local native_done = false
vim.ui.input = function(_, callback)
  callback('target')
end
vim.ui.select = function(items, options, callback)
  assert(#items == 1)
  assert(options.format_item(items[1]):match('Target note'))
  native_done = true
  callback(nil)
end
search.search_notes({ bufnr = bufnr })
assert(vim.wait(5000, function()
  return native_done
end), 'complete native note search')

local live_callbacks
local live_done = false
local updates = 0
local adapter = {
  start = function(callbacks)
    live_callbacks = callbacks
  end,
  update = function(items, state)
    updates = updates + 1
    assert(#items == 1)
    assert(items[1].title == 'Target note')
    assert(state.complete == true)
    live_done = true
  end,
}
search.live_search('note', adapter, { bufnr = bufnr, debounce_ms = 1 })
live_callbacks.on_query('discarded')
live_callbacks.on_query('target')
assert(vim.wait(5000, function()
  return live_done
end), 'complete live note search')
assert(updates == 1, 'stale live result was suppressed')
live_callbacks.on_close()

local client = assert(vim.lsp.get_client_by_id(client_id))
local response = client:request_sync('plumb/search', {
  kind = 'note',
  query = 'target',
  limit = 20,
}, 5000, bufnr)
assert(response and not response.err, response and vim.inspect(response.err) or 'no response')
assert(response.result.schemaVersion == 1)
assert(response.result.complete == true)
assert(#response.result.items == 1)
assert(response.result.items[1].title == 'Target note')
assert(response.result.items[1].location.uri:match('target%.plumb$'))

client:stop(true)
vim.fn.delete(root, 'rf')
