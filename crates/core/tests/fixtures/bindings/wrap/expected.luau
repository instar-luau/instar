local function load()
	local source: InputConfiguration,
		destination: OutputConfiguration,
		permissions: UserPermissions,
		diagnostics: ValidationDiagnostics = read_configuration(
			source_path,
			destination_path,
			processing_configuration,
			active_user_permissions,
			selected_output_format,
			validation_options
		)
	return source, destination, permissions, diagnostics
end
