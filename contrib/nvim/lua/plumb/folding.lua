local M = {}

local task_highlights = {
  DONE = '@lsp.typemod.task.completed.plumb',
  CANCELED = '@lsp.typemod.task.canceled.plumb',
}

function M.foldtext()
  local text = vim.lsp.foldtext()
  if type(text) ~= 'string' then
    return text
  end
  local state = text:match('^%s*(%u+)%s%s')
  local highlight = task_highlights[state]
  if highlight then
    return { { text, highlight } }
  end
  return text
end

return M
