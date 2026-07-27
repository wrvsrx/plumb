local config = vim.uv.fs_realpath(debug.getinfo(1, "S").source:sub(2))
local repo_root = vim.fs.dirname(vim.fs.dirname(config))
local grammar_dir = repo_root .. "/tree-sitter-plumb"
local parser_path = grammar_dir .. "/build/plumb.so"

vim.opt.runtimepath:prepend(repo_root .. "/contrib/nvim")

-- The user's init may have loaded the packaged plugin before this exrc. Reload
-- the modules after prepending the checkout so setup uses the local sources.
for name in pairs(package.loaded) do
  if name == "plumb" or vim.startswith(name, "plumb.") then
    package.loaded[name] = nil
  end
end

vim.g.plumb_nvim_auto_setup = false
require("plumb").setup({
  command = repo_root .. "/target/debug/plumb",
  codelens = { picker = "snacks" },
  search = { picker = "snacks" },
})

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
