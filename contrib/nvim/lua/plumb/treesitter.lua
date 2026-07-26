local M = {}
local warned = false

local function start(bufnr)
  local ok, error = pcall(vim.treesitter.start, bufnr, 'plumb')
  if not ok and not warned then
    warned = true
    vim.notify('plumb: tree-sitter parser unavailable: ' .. tostring(error), vim.log.levels.WARN)
  end
end

function M.setup(opts, group)
  if opts.enabled == false then
    return
  end
  if opts.parser_path then
    vim.treesitter.language.add('plumb', { path = vim.fs.normalize(opts.parser_path) })
  end
  vim.api.nvim_create_autocmd('FileType', {
    group = group,
    pattern = 'plumb',
    callback = function(args)
      start(args.buf)
    end,
  })
  for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(bufnr) and vim.bo[bufnr].filetype == 'plumb' then
      start(bufnr)
    end
  end
end

return M
