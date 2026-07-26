local M = {}

local function binary_version(command)
  local result = vim.system({ command, '--version' }, { text = true }):wait()
  if result.code ~= 0 then
    return nil, vim.trim(result.stderr or '')
  end
  return vim.trim(result.stdout or '')
end

function M.check()
  vim.health.start('plumb.nvim')
  if vim.fn.has('nvim-0.12') == 1 then
    vim.health.ok('Neovim 0.12 or newer')
  else
    vim.health.error('Neovim 0.12 or newer is required')
  end

  local config = require('plumb').config()
  local expected = require('plumb.version')
  local command = config.command or 'plumb'
  if vim.fn.executable(command) == 1 then
    local version, error = binary_version(command)
    if version then
      if version:match(vim.pesc(expected.version) .. '$') then
        vim.health.ok(version .. ' matches plumb.nvim')
      else
        vim.health.warn(string.format('%s does not match plumb.nvim %s', version, expected.version))
      end
    else
      vim.health.error('cannot run ' .. command .. ': ' .. error)
    end
  else
    vim.health.error(command .. ' is not executable')
  end

  local parser_ok, parser = pcall(vim.treesitter.language.inspect, 'plumb')
  if parser_ok and parser then
    local abi = parser._abi_version or parser.abi_version
    if abi == expected.tree_sitter_abi then
      vim.health.ok(string.format('tree-sitter plumb ABI %s', abi))
    else
      vim.health.warn(string.format(
        'tree-sitter plumb ABI %s; plugin expects ABI %s',
        tostring(abi or 'unknown'),
        expected.tree_sitter_abi
      ))
    end
  else
    vim.health.warn('tree-sitter plumb parser is not registered')
  end
  if #vim.api.nvim_get_runtime_file('queries/plumb/highlights.scm', true) > 0 then
    vim.health.ok('tree-sitter highlight queries found')
  else
    vim.health.warn('tree-sitter highlight queries are missing')
  end

  local clients = vim.lsp.get_clients({ name = 'plumb' })
  if #clients > 0 then
    vim.health.ok(string.format('%d plumb LSP client(s) active', #clients))
  else
    vim.health.info('no active plumb LSP client')
  end
end

return M
