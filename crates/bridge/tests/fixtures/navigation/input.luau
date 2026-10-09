type Record = {value: number}
local record: Record = {value = 42}
local function identity(value: Record): Record
    return value
end
do
    local record = "shadow"
    print(record)
end
return identity(record).value
