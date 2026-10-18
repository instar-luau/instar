local result=transform(records, -- Preserve  source order.
configuration)
if ready -- Validation has completed.
and permitted then process_records(records) end
local attached=transform(records,processing_configuration,active_user_permissions,selected_output_format,cancellation_token) -- Preserve source order.
emit(value) -- This comment is intentionally longer than the target width; keep this otherwise-fitting call compact.
