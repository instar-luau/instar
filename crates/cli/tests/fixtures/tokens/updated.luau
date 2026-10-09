-- semantic 😀
export type Point = { value: number }
local function adjust(value: number): number
    return value + 1
end
local object: Point = { value = 42 }
local label = "changed😀"; object.value += adjust(20)
local text = [[first
second]]
return tostring(explain(adjust(object.value))), label, text
