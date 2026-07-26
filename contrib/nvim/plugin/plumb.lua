if vim.g.loaded_plumb_nvim then
  return
end
vim.g.loaded_plumb_nvim = true

vim.filetype.add({ extension = { plumb = 'plumb' } })
