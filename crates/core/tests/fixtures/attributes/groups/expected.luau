@native
local function process(records)
	return transform(records)
end
@[native, checked]
local function validate(record: Record): boolean
	return validate_record(record)
end
@[
	native,
	checked,
	deprecated {
		use = "process_records",
		reason = "Use the validated processing entry point to retain permission checks and cancellation handling."
	}
]
local function previous(records)
	return transform(records)
end
local handler = @native function(record)
	return transform(record)
end
