do
local s=''
while running do s=s..'x' end
return s
end

do
local s=''
while running do local t=s..'x' print(t) end
end