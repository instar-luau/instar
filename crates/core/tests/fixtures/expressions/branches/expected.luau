local compact = if ready then primary else fallback
local result = if ready then
		process_records(
			records,
			processing_configuration,
			active_user_permissions,
			selected_output_format
		)
	elseif can_retry then
		retry(records)
	else
		build_fallback(records)
local checked = if
		configuration:is_ready()
		and permissions:allow_processing(active_user)
		and not cancellation_token:is_cancelled()
	then
		process_records(records)
	else
		build_fallback(records)
