local total = calculate_subtotal(records, pricing_configuration)
	+ shipping_distance * shipping_rate
	+ calculate_tax(records, taxation_configuration)
	- calculate_discount(customer, discount_configuration)
type Configuration = SourceConfiguration
	| DestinationConfiguration
	| PermissionConfiguration
	| ValidationConfiguration
	| CancellationConfiguration
type Services = ConfigurationProvider
	& PermissionProvider
	& ValidationProvider
	& CancellationProvider
	& DiagnosticsProvider
type Result = (PrimaryResult | FallbackResult) & Metadata
