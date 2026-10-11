--!nocheck
local identity: (any) -> any = function(parameter)
    return parameter
end
local annotated: number = 'incompatible'
return identity, annotated
