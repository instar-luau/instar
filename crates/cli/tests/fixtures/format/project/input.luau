--!strict
local library = require("./library")
local text = "é😀"

local function count(first: number, second: number): number
    return first + second
end

local function caller(): number
    return library.double(count(1, 2))
end

local value: library.Record = { value = caller() }

return {
    value = value,
    text = text,
    caller = caller,
    count = count,
}
