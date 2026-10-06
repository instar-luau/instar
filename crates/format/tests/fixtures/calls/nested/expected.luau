outer("head", inner(function(value)
	return {
		value = value,
	}
end, {
	enabled = true,
}), "tail")
