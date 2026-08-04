local M = {}
local states = {}
local scheduled_updates = {}

local task_highlights = {
  DONE = '@lsp.typemod.task.completed.plumb',
  CANCELED = '@lsp.typemod.task.canceled.plumb',
}

local function update_windows(bufnr)
  if vim.api.nvim_get_mode().mode:match('^i') then
    if not scheduled_updates[bufnr] then
      scheduled_updates[bufnr] = true
      vim.api.nvim_create_autocmd('InsertLeave', {
        buffer = bufnr,
        once = true,
        callback = function()
          scheduled_updates[bufnr] = nil
          if vim.api.nvim_buf_is_valid(bufnr) then
            update_windows(bufnr)
          end
        end,
      })
    end
    return
  end
  for _, winid in ipairs(vim.fn.win_findbuf(bufnr)) do
    if vim.wo[winid].foldmethod == 'expr' then
      vim._foldupdate(winid, 0, vim.api.nvim_buf_line_count(bufnr))
    end
  end
end

local function evaluate(ranges, line_count)
  local levels = {}
  local texts = {}
  for _, range in ipairs(ranges or {}) do
    local start_row = range.startLine
    local end_row = range.endLine
    if start_row >= 0 and start_row == end_row and end_row < line_count then
      levels[start_row] = (levels[start_row] or 0) + 1
      if range.collapsedText then
        texts[start_row] = range.collapsedText
      end
    end
  end
  return levels, texts
end

function M.attach(client, bufnr)
  states[bufnr] = states[bufnr] or {}
  states[bufnr].client_id = client.id
  M.refresh(bufnr)
end

function M.detach(client_id, bufnr)
  local state = states[bufnr]
  if state and state.client_id == client_id then
    states[bufnr] = nil
    update_windows(bufnr)
  end
end

function M.refresh(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local state = states[bufnr]
  if not state then
    return
  end
  local client = vim.lsp.get_client_by_id(state.client_id)
  if not client or not client:supports_method('textDocument/foldingRange', bufnr) then
    return
  end
  local changedtick = vim.api.nvim_buf_get_changedtick(bufnr)
  client:request('textDocument/foldingRange', {
    textDocument = vim.lsp.util.make_text_document_params(bufnr),
  }, function(err, ranges)
    if err or not vim.api.nvim_buf_is_valid(bufnr) then
      return
    end
    local current = states[bufnr]
    if not current or current.client_id ~= client.id then
      return
    end
    if vim.api.nvim_buf_get_changedtick(bufnr) ~= changedtick then
      return
    end
    current.levels, current.texts = evaluate(ranges, vim.api.nvim_buf_line_count(bufnr))
    update_windows(bufnr)
  end, bufnr)
end

function M.foldexpr(lnum)
  local state = states[vim.api.nvim_get_current_buf()]
  local row = (lnum or vim.v.lnum) - 1
  local extra = state and state.levels and state.levels[row]
  local native = vim.lsp.foldexpr(lnum)
  if not extra then
    return native
  end
  local level = tonumber(native:match('%d+')) or 0
  local combined = tostring(level + extra)
  if row + 1 < vim.api.nvim_buf_line_count(0) then
    return '>' .. combined
  end
  return combined
end

local function style_text(text)
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

function M.foldtext()
  local state = states[vim.api.nvim_get_current_buf()]
  local text = state and state.texts and state.texts[vim.v.foldstart - 1]
    or vim.lsp.foldtext()
  return style_text(text)
end

M._evaluate = evaluate
M._style_text = style_text

return M
