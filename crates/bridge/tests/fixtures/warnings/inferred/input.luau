--!strict
local explicit: any = 1
local inferred = explicit
local known = 1
local callback: (any) -> any = function(parameter) return parameter end
return inferred, known, callback