function object:read(index)
	if self.items[index] then
		return self.items[index].value
	end
	return nil
end
