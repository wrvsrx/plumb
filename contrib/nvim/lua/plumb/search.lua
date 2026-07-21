local M = {}

local SCHEMA_VERSION = 1
local METHOD = 'plumb/search'

local function notify(message, level)
  vim.notify('plumb: ' .. message, level or vim.log.levels.ERROR)
end

local function search_capability(client)
  local experimental = client.server_capabilities.experimental
  local search = experimental
    and experimental.plumb
    and experimental.plumb.search
  if type(search) ~= 'table' or search.method ~= METHOD then
    return nil
  end
  return search
end

local function find_client(bufnr)
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr })) do
    local capability = search_capability(client)
    if capability then
      if capability.schemaVersion ~= SCHEMA_VERSION then
        return nil, string.format(
          'unsupported search schema version %s (expected %d)',
          tostring(capability.schemaVersion),
          SCHEMA_VERSION
        )
      end
      return client
    end
  end
  return nil, 'no attached plumb LSP with structured search support'
end

local function open_location(client, location)
  vim.lsp.util.show_document(location, client.offset_encoding, { focus = true })
end

local function item_label(item)
  local parts = { item.title, item.path }
  if item.kind == 'task' then
    table.insert(parts, item.state or 'open')
    if item.due then
      table.insert(parts, 'due ' .. item.due)
    end
  end
  return table.concat(parts, '  ')
end

local function request(client, bufnr, params, callback)
  local success, request_id = client:request(METHOD, params, function(err, result)
    vim.schedule(function()
      if err then
        callback(nil, err.message or tostring(err))
      elseif not result or result.schemaVersion ~= SCHEMA_VERSION then
        callback(nil, 'invalid structured search response')
      else
        callback(result)
      end
    end)
  end, bufnr)
  if not success or not request_id then
    callback(nil, 'could not send search request')
  end
  return request_id
end

local function native_search(kind, opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local client, error = find_client(bufnr)
  if not client then
    notify(error)
    return
  end
  vim.ui.input({ prompt = opts.prompt or ('Search ' .. kind .. 's: ') }, function(query)
    if query == nil then
      return
    end
    request(client, bufnr, {
      kind = kind,
      query = query,
      filter = opts.filter,
      limit = opts.limit or 100,
    }, function(result, request_error)
      if not result then
        notify(request_error)
        return
      end
      if not result.complete then
        notify('results were truncated', vim.log.levels.WARN)
      end
      vim.ui.select(result.items, {
        prompt = opts.select_prompt or ('Select ' .. kind .. ':'),
        format_item = item_label,
      }, function(item)
        if item then
          open_location(client, item.location)
        end
      end)
    end)
  end)
end

function M.search_notes(opts)
  native_search('note', opts)
end

function M.search_tasks(opts)
  native_search('task', opts)
end

-- The adapter owns its UI and calls on_query for each changed query. It must
-- implement start(callbacks) and update(items, state); preview is optional.
function M.live_search(kind, adapter, opts)
  opts = opts or {}
  if kind ~= 'note' and kind ~= 'task' then
    error("kind must be 'note' or 'task'")
  end
  if type(adapter) ~= 'table'
    or type(adapter.start) ~= 'function'
    or type(adapter.update) ~= 'function'
  then
    error('adapter must implement start and update')
  end
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local client, client_error = find_client(bufnr)
  if not client then
    notify(client_error)
    return
  end
  local generation = 0
  local pending_request
  local closed = false

  local function cancel_pending()
    if pending_request then
      client:cancel_request(pending_request)
      pending_request = nil
    end
  end

  adapter.start({
    on_query = function(query)
      generation = generation + 1
      local current = generation
      cancel_pending()
      vim.defer_fn(function()
        if closed or current ~= generation then
          return
        end
        pending_request = request(client, bufnr, {
          kind = kind,
          query = query or '',
          filter = opts.filter,
          limit = opts.limit or 100,
        }, function(result, request_error)
          if closed or current ~= generation then
            return
          end
          pending_request = nil
          if result then
            adapter.update(result.items, { complete = result.complete })
          else
            adapter.update({}, { complete = true, error = request_error })
          end
        end)
      end, opts.debounce_ms or 80)
    end,
    on_select = function(item)
      if item then
        open_location(client, item.location)
      end
    end,
    on_preview = function(item)
      if item and type(adapter.preview) == 'function' then
        adapter.preview(item.location)
      end
    end,
    on_close = function()
      closed = true
      generation = generation + 1
      cancel_pending()
    end,
    format_item = item_label,
  })
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

return M
