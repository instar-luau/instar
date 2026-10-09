local original = 1
local occupied = 2
local function read()
    local occupied = 3
    return original + occupied
end
return read(), occupied
