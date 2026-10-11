type function endless()
    while true do end
    return types.number
end
local function consume(value: endless<>) return value end
return consume()
