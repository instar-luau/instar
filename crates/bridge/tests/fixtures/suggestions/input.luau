type Record = {value: number}
local record: Record = {value = 42}
local function identity(value: Record): Record
    return value
end
local value: Record = identity(record)
return record.value
