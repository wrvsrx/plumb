local source = debug.getinfo(1, "S").source
local config_path = vim.uv.fs_realpath(source:sub(2))

if not config_path then
  vim.notify("plumb: cannot resolve dev/nvim.lua", vim.log.levels.ERROR)
  return
end

local repo_root = vim.fs.dirname(vim.fs.dirname(config_path))
local grammar_dir = repo_root .. "/tree-sitter-plumb"
local query_path = grammar_dir .. "/queries/highlights.scm"
local parser_path = grammar_dir .. "/build/plumb.so"

local function configure()
  if not vim.uv.fs_stat(parser_path) then
    vim.notify(
      "plumb: tree-sitter parser is missing; run ./tree-sitter-plumb/build-parser.sh",
      vim.log.levels.WARN
    )
    return
  end

  local ok, err = pcall(vim.treesitter.language.add, "plumb", { path = parser_path })
  if not ok then
    vim.notify("plumb: failed to load tree-sitter parser\n" .. err, vim.log.levels.ERROR)
    return
  end

  local query = table.concat(vim.fn.readfile(query_path), "\n")
  vim.treesitter.query.set("plumb", "highlights", query)
  vim.filetype.add({ extension = { plumb = "plumb" } })

  local group = vim.api.nvim_create_augroup("plumb_project_treesitter", { clear = true })
  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = "plumb",
    callback = function(args)
      vim.treesitter.start(args.buf, "plumb")
    end,
  })

  if vim.api.nvim_buf_get_name(0):match("%.plumb$") then
    vim.bo.filetype = "plumb"
    vim.treesitter.start(0, "plumb")
  end
end

configure()
