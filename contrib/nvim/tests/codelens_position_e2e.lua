local repo = vim.fn.getcwd()
local root = vim.fn.tempname()
vim.fn.mkdir(root, 'p')
local path = root .. '/positions.plumb'
vim.fn.writefile({
  '`#{#heading} Heading',
  '`-{.task #task} Task',
  '`node{#block} Block',
  '`-{',
  '   .task #multiline',
  '  } Multiline',
  '`outer',
  '  `node{#nested} Nested',
  'Paragraph `span[text]{#inline}.',
  '`{#raw}',
  '  payload',
}, path)

local bufnr = vim.fn.bufadd(path)
vim.fn.bufload(bufnr)
local client_id = vim.lsp.start({
  name = 'plumb-codelens-position-e2e',
  cmd = { repo .. '/target/debug/plumb', 'lsp' },
  root_dir = root,
}, { bufnr = bufnr })
assert(client_id, 'start plumb LSP')
assert(vim.wait(5000, function()
  local client = vim.lsp.get_client_by_id(client_id)
  return client and client.initialized
end), 'initialize plumb LSP')

vim.lsp.codelens.enable(true, { bufnr = bufnr, client_id = client_id })
assert(vim.wait(5000, function()
  return #vim.lsp.codelens.get({ bufnr = bufnr, client_id = client_id }) == 7
end), 'receive reference CodeLenses')

local positions = vim.tbl_map(function(item)
  local start = item.lens.range.start
  return { start.line, start.character }
end, vim.lsp.codelens.get({ bufnr = bufnr, client_id = client_id }))
table.sort(positions, function(left, right)
  return left[1] < right[1] or (left[1] == right[1] and left[2] < right[2])
end)
assert(vim.deep_equal(positions, {
  { 0, 0 },
  { 1, 0 },
  { 2, 0 },
  { 3, 0 },
  { 7, 2 },
  { 8, 23 },
  { 9, 0 },
}), vim.inspect(positions))

vim.api.nvim_set_current_buf(bufnr)
vim.cmd.redraw()
local namespace = vim.api.nvim_create_namespace('nvim.lsp.codelens:' .. client_id)
local extmarks = vim.api.nvim_buf_get_extmarks(bufnr, namespace, 0, -1, { details = true })
local rows = vim.tbl_map(function(mark)
  assert(mark[4].virt_lines_above == true)
  return mark[2]
end, extmarks)
table.sort(rows)
assert(vim.deep_equal(rows, { 0, 1, 2, 3, 7, 8, 9 }), vim.inspect(rows))

local client = assert(vim.lsp.get_client_by_id(client_id))
client:stop(true)
vim.fn.delete(root, 'rf')
