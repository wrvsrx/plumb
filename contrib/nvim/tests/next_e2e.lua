local repo = vim.fn.getcwd()
vim.opt.runtimepath:prepend(repo .. '/contrib/nvim')

local next_module = require('plumb.next')
local lsp = require('plumb.lsp')
local root = vim.fn.tempname()
vim.fn.mkdir(root, 'p')
local doc = root .. '/tasks.plumb'
vim.fn.writefile({
  '`- In flight oldest',
  '',
  ' `+ task',
  '',
  ' `@ f-oldest',
  '',
  ' `= focused 2026-09-18T09:00:00Z--',
  '',
  '`- Ready low',
  '',
  ' `+ task',
  '',
  ' `@ ready-low',
  ' `= priority 1',
}, doc)

local bufnr = vim.fn.bufadd(doc)
vim.fn.bufload(bufnr)
local client_id = vim.lsp.start({
  name = 'plumb-next-e2e',
  cmd = { repo .. '/target/debug/plumb', 'lsp' },
  root_dir = root,
  capabilities = lsp.capabilities(),
}, { bufnr = bufnr })
assert(client_id, 'start plumb LSP')
assert(vim.wait(5000, function()
  local client = vim.lsp.get_client_by_id(client_id)
  return client and client.initialized
end), 'initialize plumb LSP')

local capability, capability_error = next_module.capabilities(bufnr)
assert(capability, capability_error)
assert(capability.schema_version == 1)
assert(capability.method == 'plumb/next')

local client = assert(vim.lsp.get_client_by_id(client_id))
local response
assert(vim.wait(5000, function()
  local result = client:request_sync('plumb/next', { limit = 2 }, 2000, bufnr)
  if result and not result.err and result.result.complete then
    response = result.result
    return true
  end
  return false
end, 50), 'wait for the initial workspace index')
assert(response.schemaVersion == 1)
assert(response.focusedTotal == 1, vim.inspect(response))
assert(response.candidateLimit == 2)

local items = next_module.sections(response)
assert(#items == 2, vim.inspect(items))
assert(items[1].section == 'focused')
assert(items[2].section == 'candidates')
local in_flight_label = next_module.format_item(items[1])
assert(in_flight_label:match('%[in flight%]'), in_flight_label)
assert(in_flight_label:match('focused'), in_flight_label)
assert(next_module.format_item(items[2]):match('%[ready%]'))

-- Focus a ready task through the server's code action, then unfocus it again.
next_module.toggle(items[2], { bufnr = bufnr })
assert(vim.wait(5000, function()
  local text = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), '\n')
  return text:match('`= focused [^%s]+--') ~= nil
end), 'focus writes an open interval')
local focused_result
assert(vim.wait(5000, function()
  local result = client:request_sync('plumb/next', { limit = 2 }, 2000, bufnr)
  if result and not result.err and result.result.focusedTotal == 2 then
    focused_result = result.result
    return true
  end
  return false
end, 50), 'focus moves the task into flight')

-- Re-read the shortlist so the entry carries its new focus start.
local focused_item
for _, entry in ipairs(next_module.sections(focused_result)) do
  if entry.title == items[2].title then
    focused_item = entry
  end
end
assert(focused_item and focused_item.section == 'focused', 'candidate moved into flight')
assert(next_module.format_item(focused_item):match('focused'))

next_module.toggle(focused_item, { bufnr = bufnr })
assert(vim.wait(5000, function()
  local result = client:request_sync('plumb/next', { limit = 2 }, 2000, bufnr)
  return result and not result.err and result.result.focusedTotal == 1
end, 50), 'unfocus closes the interval')

client:stop(true)

-- The command registration is part of the public surface.
require('plumb').setup({
  command = repo .. '/target/debug/plumb',
  lsp = { enabled = false },
  search = { enabled = false },
  next = { enabled = true },
})
-- exists() returns 2 for a user-defined command.
assert(vim.fn.exists(':PlumbNext') == 2, 'PlumbNext is registered')

vim.fn.delete(root, 'rf')
print('next e2e ok')
