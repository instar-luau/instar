type Mapping<
	SourceConfiguration,
	DestinationConfiguration,
	Results...
> = (SourceConfiguration) -> (DestinationConfiguration, Results...)
local function convert<
	SourceConfiguration,
	DestinationConfiguration,
	PermissionConfiguration
>(source: SourceConfiguration): DestinationConfiguration
	return source
end
