function Link(link)
  link.attributes["to"] = nil
  return link
end

function CodeBlock(code)
  local marker = code.attributes["data-plumb-marker"]
  if marker ~= nil then
    code.classes:insert(marker)
    code.attributes["data-plumb-marker"] = nil
  end
  return code
end
