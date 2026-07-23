local M = {}

local COMMAND = 'plumb.showReferences'

local function show_references(command, ctx)
  local arguments = command.arguments or {}
  local locations = arguments[3]
  if type(locations) ~= 'table' then
    vim.notify('plumb: invalid reference CodeLens payload', vim.log.levels.ERROR)
    return
  end
  if vim.tbl_isempty(locations) then
    vim.notify('plumb: no references', vim.log.levels.INFO)
    return
  end
  local client = vim.lsp.get_client_by_id(ctx.client_id)
  if not client then
    vim.notify('plumb: CodeLens client is no longer available', vim.log.levels.ERROR)
    return
  end
  local items = vim.lsp.util.locations_to_items(locations, client.offset_encoding)
  vim.fn.setqflist({}, ' ', { title = 'plumb references', items = items })
  vim.cmd.copen()
end

function M.setup()
  vim.lsp.commands[COMMAND] = show_references
end

return M
