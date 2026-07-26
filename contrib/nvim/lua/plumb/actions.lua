local M = {}

function M.format(opts)
  opts = opts or {}
  vim.lsp.buf.format({ bufnr = opts.bufnr or 0, name = 'plumb', async = opts.async })
end

function M.rename()
  vim.lsp.buf.rename()
end

function M.code_action()
  vim.lsp.buf.code_action()
end

function M.references()
  vim.lsp.codelens.run()
end

function M.task(action)
  local titles = { complete = 'Complete task', cancel = 'Cancel task' }
  local title = titles[action]
  if not title then
    error("task action must be 'complete' or 'cancel'")
  end
  vim.lsp.buf.code_action({
    apply = true,
    filter = function(candidate)
      return candidate.title == title
    end,
  })
end

return M
