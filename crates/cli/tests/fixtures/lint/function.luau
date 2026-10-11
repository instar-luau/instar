type function numbertype()
    return types.number
end
local function consume(value: numbertype<>): number
    return value
end
return consume()
