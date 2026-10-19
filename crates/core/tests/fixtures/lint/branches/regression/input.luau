if previous then
    previous.distance = math.min(previous.distance, distance)
else
    desired[key] = { x = x, y = y, z = z, distance = distance }
    population += 1
end
