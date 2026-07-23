local M = {}

local COMMAND = 'plumb.showReferences'
local config = { picker = 'quickfix' }

local function open_quickfix(items, title)
  vim.fn.setqflist({}, ' ', { title = title, items = items })
  vim.cmd.copen()
end

local function open_snacks(items, title)
  local ok, snacks = pcall(require, 'snacks')
  if not ok or type(snacks.picker) ~= 'table' or type(snacks.picker.pick) ~= 'function' then
    vim.notify('plumb: Snacks picker is unavailable; using quickfix', vim.log.levels.WARN)
    open_quickfix(items, title)
    return
  end
  local picker_items = vim.tbl_map(function(item)
    return {
      text = item.filename .. ' ' .. (item.text or ''),
      file = item.filename,
      pos = { item.lnum, item.col - 1 },
      end_pos = item.end_lnum and item.end_col and { item.end_lnum, item.end_col - 1 } or nil,
      line = item.text,
    }
  end, items)
  snacks.picker.pick({
    title = title,
    items = picker_items,
    format = 'file',
    preview = 'file',
    confirm = 'jump',
    auto_confirm = true,
    jump = { tagstack = true, reuse_win = true },
  })
end

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
  local title = command.title or 'plumb references'
  if config.picker == 'snacks' then
    open_snacks(items, title)
  else
    open_quickfix(items, title)
  end
end

function M.setup(opts)
  opts = opts or {}
  local picker = opts.picker or 'quickfix'
  if picker ~= 'quickfix' and picker ~= 'snacks' then
    error("picker must be 'quickfix' or 'snacks'")
  end
  config = { picker = picker }
  vim.lsp.commands[COMMAND] = show_references
end

return M
