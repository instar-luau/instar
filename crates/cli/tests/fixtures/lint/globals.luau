local inferred = external
local annotated: any = external
local identity: (any) -> any = function(parameter)
    return parameter
end
local function typed(parameter: number): number
    return parameter
end
local mismatch: number = 'not a number'
return inferred, annotated, identity, typed, mismatch
