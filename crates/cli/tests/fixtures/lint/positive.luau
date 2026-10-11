do
    print(missing)
end
do
    print(gcinfo())
end
do
    local function create()
        created = 1
        print(created)
    end
    create()
end
do
    local shadow = 1
    do
        local shadow = 2
        print(shadow)
    end
    print(shadow)
end
do
    print(1) print(2)
end
do
local continued = 1 +
2
    print(continued)
end
do
    local unused = 1
end
do
    local function unusedfunction() return 1 end
end
do
    local module = require('./module')
end
do
    math = {}
end
do
    local _ = 1
    print(_)
end
do
    while true do
        if true then break else break end
        print('unreachable')
    end
end
do
    if type(1) == 'integer' then print(1) end
end
do
    for index = 8, 1 do print(index) end
end
do
    local first, second = 1
    print(first, second)
end
do
    local function partial(flag: boolean)
        if flag then return 1 end
    end
    print(partial(true))
end
do
    local duplicate, duplicate = 1, 2
    print(duplicate)
end
do
    print(string.format('%qz%'))
end
do
    print({key = 1, key = 2})
end
do
    local uninitialized
    print(uninitialized)
end
do
    local object = {}
    function object.method() return 1 end
    function object.method() return 2 end
    print(object.method())
end
do
    print(getfenv(1))
end
do
    local array = {1, 2}
    table.insert(array, 0, 3)
    print(array)
end
do
    local flag = true
    if flag then print(1) elseif flag then print(2) end
end
do
    local flag = true
    print(flag and false or 2)
end
do
    print(9007199254740993)
end
do
    local first, second = 1, 2
    print(not first == second)
end
