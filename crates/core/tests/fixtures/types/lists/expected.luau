type Result = Container<
	SourceConfiguration,
	DestinationConfiguration,
	PermissionConfiguration,
	ValidationConfiguration,
	CancellationConfiguration
>
type Callback = () -> (
	SourceConfiguration,
	DestinationConfiguration,
	PermissionConfiguration,
	ValidationConfiguration,
	CancellationConfiguration
)
type Processor = (
	source: SourceConfiguration,
	destination: DestinationConfiguration,
	permissions: PermissionConfiguration
) -> Result
