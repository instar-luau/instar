local source: SourceConfiguration,
	destination: DestinationConfiguration,
	diagnostics: ConfigurationDiagnostics<SourceConfiguration, DestinationConfiguration>
		= read_configuration(source_path, destination_path, validation_options)
