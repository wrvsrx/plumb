local M = {}

local decoration_attributes = {
  'bold',
  'italic',
  'reverse',
  'standout',
  'strikethrough',
  'underline',
  'undercurl',
  'underdashed',
  'underdotted',
  'underdouble',
}

local function task_fold_highlight(decoration)
  local highlight = vim.api.nvim_get_hl(0, { name = 'Comment', link = false })
  if decoration then
    local diagnostic = vim.api.nvim_get_hl(0, { name = decoration, link = false })
    for _, attribute in ipairs(decoration_attributes) do
      if diagnostic[attribute] ~= nil then
        highlight[attribute] = diagnostic[attribute]
      end
    end
  end
  return highlight
end

local function set_default_highlight(group, highlight, reset)
  if not reset then
    highlight.default = true
  end
  vim.api.nvim_set_hl(0, group, highlight)
end

local function configure_task_highlights(reset)
  set_default_highlight('@lsp.typemod.task.completed.plumb', {
    link = 'Comment',
  }, reset)
  set_default_highlight('@lsp.typemod.task.canceled.plumb', {
    link = 'Comment',
  }, reset)
  local fold_highlights = {
    PlumbTaskFoldWaiting = 'DiagnosticInfo',
    PlumbTaskFoldBlocked = 'DiagnosticHint',
    PlumbTaskFoldDone = 'Comment',
    PlumbTaskFoldConflicted = 'DiagnosticWarn',
  }
  for group, link in pairs(fold_highlights) do
    set_default_highlight(group, {
      link = link,
    }, reset)
  end
  set_default_highlight(
    'PlumbTaskFoldCanceled',
    task_fold_highlight('DiagnosticDeprecated'),
    reset
  )
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
  local reset_default_highlights = opts.reset_default_highlights == true
  configure_task_highlights(reset_default_highlights)
  vim.api.nvim_create_autocmd('ColorScheme', {
    group = group,
    callback = function()
      configure_task_highlights(reset_default_highlights)
    end,
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
    end,
  })
  vim.lsp.enable('plumb')
end

return M
