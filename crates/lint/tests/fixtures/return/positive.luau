do
local function f(x) if x then return 1 else return 2 end end
return f
end

do
local function f(x) if x then print(1) else return 2 end end
return f
end