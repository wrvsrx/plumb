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
      require('plumb.folding').attach(client, args.buf)
      if opts.on_attach then
        opts.on_attach(client, args.buf)
      end
      if opts.codelens ~= false and client:supports_method('textDocument/codeLens') then
        vim.lsp.codelens.enable(true, { bufnr = args.buf, client_id = client.id })
      end
    end,
  })
  vim.api.nvim_create_autocmd('LspDetach', {
    group = group,
    callback = function(args)
      require('plumb.folding').detach(args.data.client_id, args.buf)
    end,
  })
  vim.api.nvim_create_autocmd('LspNotify', {
    group = group,
    callback = function(args)
      if
        args.data.method == 'textDocument/didOpen'
        or args.data.method == 'textDocument/didChange'
      then
        require('plumb.folding').refresh(args.buf)
      end
    end,
  })
  vim.lsp.enable('plumb')
end

return M
