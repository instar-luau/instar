--!strict

export type Record = { value: number }

local function double(value: number): number
    return value * 2
end

return { double = double }
