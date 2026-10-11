--!strict

local library = require("./library")
local input = require("./input")

local value: library.Record = input.value

return library.double(value.value)
