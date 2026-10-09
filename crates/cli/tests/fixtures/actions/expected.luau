local value = 1
if value > 0 then
    ;
end
local object = { value = 1 }
object.value = object.value
value -- preserve this comment
    = value
value --[[preserve this block comment]] = value
return value
