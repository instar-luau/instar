for index,record in iterate_records(source_records,processing_configuration,active_user_permissions,selected_output_format,cancellation_token) do process(record) end
while queue:has_pending_records() and permissions:allow_processing(active_user) and not cancellation_token:is_cancelled() do process_next() end
repeat process_next() until queue:is_empty() or cancellation_token:is_cancelled() or permissions:processing_revoked(active_user)
