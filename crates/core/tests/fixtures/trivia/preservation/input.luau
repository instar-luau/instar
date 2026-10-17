f(--[[keep]])
f(1 --[[argument]],2)
local t={1 --[[field]],2}
f() --[[tail]]
f() --[[statement]]; f()
f(-- empty
)
f(1 -- argument
,2)
local lines={1 -- field
,2}
f() -- tail
f() -- statement
; f()
do
f() --[[nested]]; f()
end
