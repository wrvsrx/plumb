local M = {}

local task_highlights = {
  ['[~]'] = 'PlumbTaskFoldWaiting',
  ['[=]'] = 'PlumbTaskFoldBlocked',
  ['[o]'] = 'PlumbTaskFoldDone',
  ['[x]'] = 'PlumbTaskFoldCanceled',
  ['[ox]'] = 'PlumbTaskFoldConflicted',
}

function M.foldtext()
  local text = vim.lsp.foldtext()
  if type(text) ~= 'string' then
    return text
  end
  local state = text:match('^%s*(`task%s+%b[])%s*')
  state = state and state:match('(%b[])$')
  local highlight = task_highlights[state]
  if highlight then
    return { { text, highlight } }
  end
  return text
end

return M
