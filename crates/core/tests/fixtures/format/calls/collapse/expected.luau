local result = outer(first, inner(second, third))

dispatch(function(value)
	return value + 1
end, "tail")
dispatch({
	value = 1,
}, "tail")

dispatch("head", function(value)
	return value
end)
dispatch("head", {
	enabled = true,
})

dispatch("head", function(value)
	return value * 2
end, "tail")
dispatch("head", {
	enabled = true,
}, "tail")

dispatch(function(value)
	return value
end, "between", function(value)
	return -value
end, "tail")
dispatch({
	first = 1,
}, "between", {
	second = 2,
}, "tail")

outer("head", inner(function(value)
	return {
		value = value,
	}
end, {
	enabled = true,
}), "tail")
