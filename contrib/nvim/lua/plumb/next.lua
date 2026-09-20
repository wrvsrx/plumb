local M = {}

local SCHEMA_VERSION = require('plumb.version').next_schema
local METHOD = 'plumb/next'

M.METHOD = METHOD
M.SCHEMA_VERSION = SCHEMA_VERSION

local SECTION_LABELS = { focused = 'in flight', candidates = 'ready' }

local function notify(message, level)
  vim.notify('plumb: ' .. message, level or vim.log.levels.ERROR)
end

local function next_capability(client)
  local experimental = client.server_capabilities.experimental
  local capability = experimental
    and experimental.plumb
    and experimental.plumb.next
  if type(capability) ~= 'table' or capability.method ~= METHOD then
    return nil
  end
  return capability
end

local function find_client(bufnr)
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr })) do
    local capability = next_capability(client)
    if capability then
      if capability.schemaVersion ~= SCHEMA_VERSION then
        return nil, string.format(
          'unsupported next schema version %s (expected %d)',
          tostring(capability.schemaVersion),
          SCHEMA_VERSION
        )
      end
      return client
    end
  end
  return nil, 'no attached plumb LSP with a next shortlist'
end

-- An RFC 3339 instant as a Unix timestamp. Both sides of the later subtraction
-- are interpreted as local wall-clock time and then shifted to UTC by the
-- explicit offset, so the resulting age is correct in any timezone.
local function parse_instant(value)
  local year, month, day, hour, minute, second, sign, offset_hours, offset_minutes =
    value:match('^(%d%d%d%d)-(%d%d)-(%d%d)T(%d%d):(%d%d):(%d%d)([+-])(%d%d):(%d%d)$')
  if not year then
    return nil
  end
  local stamp = os.time({
    year = tonumber(year),
    month = tonumber(month),
    day = tonumber(day),
    hour = tonumber(hour),
    min = tonumber(minute),
    sec = tonumber(second),
    isdst = false,
  })
  local offset = tonumber(offset_hours) * 3600 + tonumber(offset_minutes) * 60
  if sign == '+' then
    return stamp - offset
  end
  return stamp + offset
end

local function format_span(seconds)
  if seconds < 60 then
    return 'just now'
  elseif seconds < 3600 then
    return string.format('%dm', math.floor(seconds / 60))
  elseif seconds < 86400 then
    local hours = math.floor(seconds / 3600)
    local minutes = math.floor((seconds % 3600) / 60)
    if minutes == 0 then
      return string.format('%dh', hours)
    end
    return string.format('%dh %dm', hours, minutes)
  end
  local days = math.floor(seconds / 86400)
  local hours = math.floor((seconds % 86400) / 3600)
  if hours == 0 then
    return string.format('%dd', days)
  end
  return string.format('%dd %dh', days, hours)
end

-- How long a task has been marked as in flight, never how long anyone worked on
-- it. A future start shows the absolute instant instead of a negative age.
local function focus_age(item)
  if type(item.focusedSince) ~= 'string' or item.focusedSince == '' then
    return nil
  end
  local start = parse_instant(item.focusedSince)
  if not start then
    return 'focused since ' .. item.focusedSince
  end
  local age = os.time() - start
  if age < 0 then
    return 'focused (future start) ' .. item.focusedSince
  end
  return 'focused ' .. format_span(age)
end

-- Flatten the two server sections into one picker list, keeping `section` so the
-- label can say which list an entry came from.
function M.sections(result)
  local items = {}
  for _, item in ipairs((result or {}).focused or {}) do
    item.section = 'focused'
    table.insert(items, item)
  end
  for _, item in ipairs((result or {}).candidates or {}) do
    item.section = 'candidates'
    table.insert(items, item)
  end
  return items
end

function M.format_item(item)
  local parts = { '[' .. (SECTION_LABELS[item.section] or item.section or '?') .. ']', item.title }
  if item.path then
    table.insert(parts, item.path)
  end
  if item.state and item.state ~= 'ready' then
    table.insert(parts, item.state)
  end
  local age = focus_age(item)
  if age then
    table.insert(parts, age)
  end
  return table.concat(parts, '  ')
end

local function open_location(client, location)
  vim.lsp.util.show_document(location, client.offset_encoding, { focus = true })
end

-- Focus/Unfocus reuses the task code actions the server already offers, so the
-- client never builds its own document edit.
local function apply_focus_action(client, location, wanted)
  local bufnr = vim.uri_to_bufnr(location.uri)
  vim.fn.bufload(bufnr)
  local responses = vim.lsp.buf_request_sync(bufnr, 'textDocument/codeAction', {
    textDocument = { uri = location.uri },
    range = location.range,
    context = { diagnostics = {}, only = { 'quickfix' } },
  }, 2000)
  for _, response in pairs(responses or {}) do
    for _, action in ipairs(response.result or {}) do
      if action.title == wanted and action.edit then
        vim.lsp.util.apply_workspace_edit(action.edit, client.offset_encoding)
        return true
      end
    end
  end
  return false, wanted .. ' is not available for this task'
end

function M.capabilities(bufnr)
  local client, error = find_client(bufnr or vim.api.nvim_get_current_buf())
  if not client then
    return nil, error
  end
  return {
    client_id = client.id,
    method = METHOD,
    schema_version = SCHEMA_VERSION,
  }
end

-- Toggle the in-flight mark on one shortlist entry.
function M.toggle(item, opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local client, client_error = find_client(bufnr)
  if not client then
    notify(client_error)
    return
  end
  local unfocus = type(item.focusedSince) == 'string'
  local ok, toggle_error = apply_focus_action(
    client,
    item.location,
    unfocus and 'Unfocus task' or 'Focus task'
  )
  if not ok then
    notify(toggle_error)
    return
  end
  vim.notify(
    string.format('plumb: %s %s', unfocus and 'unfocused' or 'focused', item.title),
    vim.log.levels.INFO
  )
end

-- Request the shared shortlist, then let the user pick an entry and jump to it.
function M.open(opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local client, client_error = find_client(bufnr)
  if not client then
    notify(client_error)
    return
  end
  client:request(METHOD, { limit = opts.limit or 3, cursor = opts.cursor }, function(err, result)
    vim.schedule(function()
      if err then
        notify(err.message or tostring(err))
        return
      end
      if not result or result.schemaVersion ~= SCHEMA_VERSION then
        notify('invalid next shortlist response')
        return
      end
      if not result.complete then
        notify(
          'workspace index is incomplete; the shortlist may be missing tasks',
          vim.log.levels.WARN
        )
      end
      for _, skipped in ipairs(result.skippedInvalid or {}) do
        notify(
          string.format(
            'skipped invalid focus history: %s (%s)',
            skipped.title,
            table.concat(skipped.codes or {}, ', ')
          ),
          vim.log.levels.WARN
        )
      end
      local items = M.sections(result)
      if #items == 0 then
        notify('no task is in flight and no task is ready', vim.log.levels.INFO)
        return
      end
      vim.ui.select(items, {
        prompt = 'plumb next:',
        format_item = M.format_item,
        picker = opts.picker,
      }, function(item)
        if not item then
          return
        end
        if opts.on_select then
          opts.on_select(item)
        else
          open_location(client, item.location)
        end
        if opts.toggle then
          M.toggle(item, { bufnr = bufnr })
        end
      end)
    end)
  end, bufnr)
end

return M
