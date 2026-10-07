--!strict
local collection: any = {}
local key, value = 1, 2
for key: any, value in collection do
print(key, value)
for key: any, value in collection do print(key, value) end
end
for key: any, value: any in collection do print(key, value) end
for key, value in {1, 2} do print(key, value) end
return key, value