local function combine(text: string, count: number, ...: boolean): string
    return text
end
local function literal(text: "😀", count: number): number
    return count
end
local receiver = {}
function receiver:method(text: string, count: number): number
    return count
end
local choose = (nil :: any) :: ((text: string) -> string) & ((value: number, base: number) -> number)
local first = combine("😀", 2, true, false)
local nested = combine(string.format("%s,%s", "a", "b"), 2)
local unicode = literal("😀", 1)
local method = receiver:method("x", 1)
local explicit = receiver.method(receiver, "x", 1)
local overload = choose(1, 2)
local function transform(callback: (string, string) -> string, value: number): number
    return value
end
local transformed = transform(function(left, right) return left .. right end, 1)
local callable = setmetatable({}, { __call = function(self, value: number): number return value end })
local called = callable(1)
