do
local a,b=1,2
a=b
b=a
return a+b
end

do
local a,b=1,2
a,b=b,a
return a+b
end