if vim.g.plumb_nvim_auto_setup == false or vim.g.plumb_nvim_setup then
  return
end

require('plumb').setup(vim.g.plumb_nvim_config or {})
