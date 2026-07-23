local M = {}

local function attached_root(bufnr)
  for _, client in pairs(vim.lsp.get_clients({ bufnr = bufnr, name = 'plumb' })) do
    if client.config.root_dir then
      return client.config.root_dir
    end
  end
end

function M.open(opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local root = opts.root or attached_root(bufnr) or vim.fs.root(bufnr, { '.git' }) or vim.fn.getcwd()
  local command = { opts.command or 'plumb', 'graph', '--root', root }
  if opts.current ~= false then
    local current = vim.api.nvim_buf_get_name(bufnr)
    if current ~= '' and vim.bo[bufnr].filetype == 'plumb' then
      vim.list_extend(command, { '--current', current })
    end
  end
  return vim.system(command, { detach = true }, function(result)
    if result.code ~= 0 then
      vim.schedule(function()
        local message = result.stderr ~= '' and result.stderr or 'graph process exited unexpectedly'
        vim.notify('plumb: ' .. vim.trim(message), vim.log.levels.ERROR)
      end)
    end
  end)
end

function M.setup(opts)
  opts = opts or {}
  vim.api.nvim_create_user_command('PlumbGraph', function(command)
    M.open({
      command = opts.command,
      root = opts.root,
      current = not command.bang,
    })
  end, {
    bang = true,
    desc = 'Open the plumb workspace graph; use ! to omit the current note',
  })
end

return M
