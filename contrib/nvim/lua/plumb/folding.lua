local M = {}

local task_highlights = {
  ['+'] = '@lsp.typemod.task.completed.plumb',
  ['+x'] = '@lsp.typemod.task.completed.plumb',
  x = '@lsp.typemod.task.canceled.plumb',
}

function M.foldtext()
  local text = vim.lsp.foldtext()
  if type(text) ~= 'string' then
    return text
  end
  local state = text:match('^%s*([+x]+)%s%s')
  local highlight = task_highlights[state]
  if highlight then
    return { { text, highlight } }
  end
  return text
end

return M
