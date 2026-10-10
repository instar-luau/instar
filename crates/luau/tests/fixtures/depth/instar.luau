local value = {}

for index = 1, 129 do
    value = { nested = value }
end

return { checking = value }
