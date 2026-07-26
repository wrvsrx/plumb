local M = {}

function M.select(items, opts, callback)
  local ok, snacks = pcall(require, 'snacks')
  if not ok or type(snacks.picker) ~= 'table' or type(snacks.picker.pick) ~= 'function' then
    return false, 'Snacks picker is unavailable'
  end
  local picker_items = vim.tbl_map(function(item)
    return {
      text = opts.format_item(item),
      file = vim.uri_to_fname(item.location.uri),
      pos = { item.location.range.start.line + 1, item.location.range.start.character },
      item = item,
    }
  end, items)
  snacks.picker.pick({
    title = opts.prompt,
    items = picker_items,
    format = 'file',
    preview = 'file',
    confirm = function(picker, item)
      picker:close()
      callback(item and item.item or nil)
    end,
  })
  return true
end

return M
