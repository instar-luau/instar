-- first documentation line
-- second documentation line
local banner = "𝑥"; local object = {
    value = 1,
    nested = {
        flag = true,
    },
}
local function choose(enabled)
    if enabled then
        return object.nested
    end
    return object
end
local text = [[first
second𝑥]]
--[[block
comment
]]
return choose(true), text, banner
