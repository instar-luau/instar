local text = "𝑥"; local count = 1
local annotated: number = 2
local double: (number) -> number = function(value)
    return value * 2
end
local result = double(count)
return text, result, annotated
