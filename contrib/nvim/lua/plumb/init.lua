local M = {}

local defaults = {
  command = 'plumb',
  lsp = { enabled = true, root_markers = { '.plumb', '.git' } },
  codelens = { enabled = true, picker = 'quickfix' },
  search = { enabled = true, picker = 'native', task_filter = 'state == "ready"' },
}

local config = vim.deepcopy(defaults)
local commands = {
  'PlumbNotes',
  'PlumbTasks',
}

local function command(name, callback, desc)
  pcall(vim.api.nvim_del_user_command, name)
  vim.api.nvim_create_user_command(name, callback, { desc = desc })
end

local function clear_commands()
  for _, name in ipairs(commands) do
    pcall(vim.api.nvim_del_user_command, name)
  end
end

function M.setup(opts)
  if vim.fn.has('nvim-0.12') ~= 1 then
    error('plumb.nvim requires Neovim 0.12 or newer')
  end
  config = vim.tbl_deep_extend('force', vim.deepcopy(defaults), opts or {})
  local group = vim.api.nvim_create_augroup('PlumbNvim', { clear = true })
  clear_commands()
  vim.filetype.add({ extension = { plumb = 'plumb' } })

  local lsp = vim.tbl_deep_extend('force', {
    command = config.command,
    codelens = config.codelens.enabled,
  }, config.lsp)
  require('plumb.lsp').setup(lsp, group)
  if config.codelens.enabled then
    require('plumb.codelens').setup({ picker = config.codelens.picker })
  end
  if config.search.enabled then
    command('PlumbNotes', function()
      require('plumb.search').search_notes({ picker = config.search.picker })
    end, 'Search plumb notes')
    command('PlumbTasks', function()
      require('plumb.search').search_tasks({
        picker = config.search.picker,
        filter = config.search.task_filter,
      })
    end, 'Search plumb tasks')
  end
  return M
end

function M.config()
  return vim.deepcopy(config)
end

function M.health()
  require('plumb.health').check()
end

return M
