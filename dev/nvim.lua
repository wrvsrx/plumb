local config = vim.uv.fs_realpath(debug.getinfo(1, "S").source:sub(2))
local repo_root = vim.fs.dirname(vim.fs.dirname(config))
local grammar_dir = repo_root .. "/tree-sitter-plumb"
local parser_path = grammar_dir .. "/build/plumb.so"

if vim.uv.fs_stat(parser_path) then
  vim.treesitter.language.add("plumb", { path = parser_path })
  for _, query in ipairs({ "highlights", "folds", "indents", "textobjects", "injections" }) do
    vim.treesitter.query.set(
      "plumb",
      query,
      table.concat(vim.fn.readfile(grammar_dir .. "/queries/" .. query .. ".scm"), "\n")
    )
  end

  vim.api.nvim_create_autocmd("FileType", {
    pattern = "plumb",
    callback = function(args)
      vim.treesitter.start(args.buf, "plumb")
    end,
  })
else
  vim.notify("plumb: run ./tree-sitter-plumb/build-parser.sh", vim.log.levels.WARN)
end

vim.filetype.add({ extension = { plumb = "plumb" } })

local capabilities = vim.lsp.protocol.make_client_capabilities()
capabilities.workspace.workspaceEdit.documentChanges = true
capabilities.workspace.workspaceEdit.resourceOperations = { "rename" }
-- Neovim disables workspace/didChangeWatchedFiles by default on Linux. Opt in
-- for local plumb-ls testing; install inotify-tools for the inotify backend.
capabilities.workspace.didChangeWatchedFiles =
  capabilities.workspace.didChangeWatchedFiles or {}
capabilities.workspace.didChangeWatchedFiles.dynamicRegistration = true
capabilities.workspace.didChangeWatchedFiles.relativePatternSupport = true

vim.lsp.config["plumb-ls"] = {
  cmd = { repo_root .. "/target/debug/plumb-ls" },
  filetypes = { "plumb" },
  root_dir = repo_root,
  capabilities = capabilities,
}

local function set_plumb_semantic_highlights()
  vim.api.nvim_set_hl(0, "@lsp.typemod.task.completed.plumb", {
    link = "Comment",
    default = true,
  })
end

set_plumb_semantic_highlights()

vim.api.nvim_create_autocmd("ColorScheme", {
  callback = set_plumb_semantic_highlights,
})

vim.lsp.enable("plumb-ls")
