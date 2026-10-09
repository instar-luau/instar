local dependency = require("./dependency")
local alias = dependency.target

local function caller()
    local text = "😀"; alias()
    alias()
    local function nested()
        alias()
    end
    nested()
    return text
end

local function shadow()
    local alias = function() return 1 end
    return alias()
end

caller()
shadow()
return caller
