local M = {}

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
    end,
  })
  vim.lsp.enable('plumb')
end

return M
