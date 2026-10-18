local compact = pipeline:select(records):export(output)
local result = pipeline
	:select(active_records)
	:transform(
		processing_configuration,
		active_user_permissions,
		selected_output_format,
		cancellation_token,
		validation_options
	)
	:validate(validation_rules)
	:export(selected_output_format)
local success, result = pcall(function()
	return process_records(
		records,
		processing_configuration,
		active_user_permissions,
		selected_output_format,
		cancellation_token
	)
end)
