local known = 1
print(known)
local function used() return 1 end
print(used())
local module = require('./module')
print(module.required(1, 2))
local placeholder = 1
print(placeholder)
for index = 1, 8 do print(index) end
local first, second = 1, 2
print(first, second)
local function total(flag: boolean)
    if flag then return 1 else return 2 end
end
print(total(true))
print(string.format('%s', 'safe'))
print({key = 1, other = 2})
local initialized = nil
print(initialized)
local object = {}
function object.first() return 1 end
function object.second() return 2 end
print(object.first(), object.second())
local array = {1, 2}
table.insert(array, 3)
print(array)
if first == 1 then print(1) elseif second == 2 then print(2) end
print(if first == 1 then false else 2)
print(9007199254740992)
print(first ~= second)
