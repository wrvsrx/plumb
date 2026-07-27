local M = {}

local function configure_task_highlights()
  vim.api.nvim_set_hl(0, '@lsp.typemod.task.completed.plumb', {
    default = true,
    link = 'Comment',
  })
  vim.api.nvim_set_hl(0, '@lsp.typemod.task.canceled.plumb', {
    default = true,
    link = 'Comment',
  })
end

local function configure_folding(bufnr, winid)
  if vim.api.nvim_win_get_buf(winid) ~= bufnr then
    return
  end
  vim.wo[winid][0].foldmethod = 'expr'
  vim.wo[winid][0].foldexpr = 'v:lua.vim.lsp.foldexpr()'
  vim.wo[winid][0].foldtext = 'v:lua.vim.lsp.foldtext()'
  vim.wo[winid][0].foldlevel = 99
end

local function has_folding_client(bufnr)
  return vim.iter(vim.lsp.get_clients({ bufnr = bufnr, name = 'plumb' })):any(function(client)
    return client:supports_method('textDocument/foldingRange')
  end)
end

function M.capabilities()
  local capabilities = vim.lsp.protocol.make_client_capabilities()
  capabilities.workspace.workspaceEdit.documentChanges = true
  capabilities.workspace.workspaceEdit.resourceOperations = { 'rename' }
  capabilities.workspace.didChangeWatchedFiles = capabilities.workspace.didChangeWatchedFiles or {}
  capabilities.workspace.didChangeWatchedFiles.dynamicRegistration = true
  capabilities.workspace.didChangeWatchedFiles.relativePatternSupport = true
  return capabilities
end

function M.setup(opts, group)
  if opts.enabled == false then
    vim.lsp.enable('plumb', false)
    return
  end
  configure_task_highlights()
  vim.api.nvim_create_autocmd('ColorScheme', {
    group = group,
    callback = configure_task_highlights,
  })
  local config = {
    cmd = opts.cmd or { opts.command or 'plumb', 'lsp' },
    filetypes = { 'plumb' },
    root_markers = opts.root_markers or { '.plumb', '.git' },
    capabilities = vim.tbl_deep_extend('force', M.capabilities(), opts.capabilities or {}),
  }
  if opts.settings then
    config.settings = opts.settings
  end
  vim.lsp.config('plumb', config)
  vim.api.nvim_create_autocmd('LspAttach', {
    group = group,
    callback = function(args)
      local client = vim.lsp.get_client_by_id(args.data.client_id)
      if not client or client.name ~= 'plumb' then
        return
      end
      if opts.on_attach then
        opts.on_attach(client, args.buf)
      end
      if opts.codelens ~= false and client:supports_method('textDocument/codeLens') then
        vim.lsp.codelens.enable(true, { bufnr = args.buf, client_id = client.id })
      end
      if opts.folding ~= false and client:supports_method('textDocument/foldingRange') then
        for _, winid in ipairs(vim.fn.win_findbuf(args.buf)) do
          configure_folding(args.buf, winid)
        end
      end
    end,
  })
  if opts.folding ~= false then
    vim.api.nvim_create_autocmd('BufWinEnter', {
      group = group,
      pattern = '*.plumb',
      callback = function(args)
        if has_folding_client(args.buf) then
          configure_folding(args.buf, vim.api.nvim_get_current_win())
        end
      end,
    })
  end
  vim.lsp.enable('plumb')
end

return M
