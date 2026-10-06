dispatch("head", function(value)
	return value * 2
end, "tail")
dispatch("head", {
	enabled = true,
}, "tail")
