--!strict
local loader = require
local imported = loader('./not-on-disk')
local number: number = imported
return number