local function values(...)
	return ...
end
local first, second = values(1, 2)
local single = (values(1, 2))
local array = { values(1, 2) }
local truncated = { (values(1, 2)) }
local result = -2 ^ 2 + 2 ^ 3 ^ 2
local concatenated = "a" .. "b" .. "c"
return first, values(second, single)
