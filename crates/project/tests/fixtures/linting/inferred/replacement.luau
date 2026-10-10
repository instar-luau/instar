local callback: (any)->any = function(parameter) return parameter end
local value=callback(1)
return value,callback