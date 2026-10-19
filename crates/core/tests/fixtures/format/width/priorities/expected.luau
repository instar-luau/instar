local result = combine(
	calculate_primary_contribution(records, configuration)
	+ calculate_secondary_contribution(records, configuration)
	+ calculate_fallback_contribution(records, configuration),
	active_user_permissions,
	selected_output_format
)
local label = "A literal that is intentionally longer than the target line width and must remain completely intact."
