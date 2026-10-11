do
local dest={}
for k,v in pairs(src) do dest[k]=v end
return dest
end

do
local dest={}
for k,v in pairs(src) do dest[k]=v print(k) end
return dest
end