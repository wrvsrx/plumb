local config = vim.uv.fs_realpath(debug.getinfo(1, "S").source:sub(2))
local repo_root = vim.fs.dirname(vim.fs.dirname(config))
local grammar_dir = repo_root .. "/tree-sitter-plumb"
local parser_path = grammar_dir .. "/build/plumb.so"

if not vim.uv.fs_stat(parser_path) then
  vim.notify("plumb: run ./tree-sitter-plumb/build-parser.sh", vim.log.levels.WARN)
  return
end

vim.treesitter.language.add("plumb", { path = parser_path })
vim.treesitter.query.set(
  "plumb",
  "highlights",
  table.concat(vim.fn.readfile(grammar_dir .. "/queries/highlights.scm"), "\n")
)

vim.filetype.add({ extension = { plumb = "plumb" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "plumb",
  callback = function(args)
    vim.treesitter.start(args.buf, "plumb")
  end,
})
