declare extern type debug with
	info: typeof(debug.info)
end

declare extern type utf8 with
	codes: typeof(utf8.codes)
	len: typeof(utf8.len)
	graphemes: (string, number?, number?) -> (() -> (number, number))
end

declare extern type Axes with
	new: (...(Enum.Axis | Enum.NormalId)) -> Axes
end

declare extern type Faces with
	new: (...Enum.NormalId) -> Faces
end

declare extern type BrickColor with
	Random: () -> BrickColor
	new: (() -> BrickColor)
		& ((value: number) -> BrickColor)
		& ((red: number, green: number) -> BrickColor)
		& ((red: number, green: number, blue: number) -> BrickColor)
		& ((name: string) -> BrickColor)
		& ((color: Color3) -> BrickColor)
end

declare extern type Vector2 with
	new: (x: number?, y: number?) -> Vector2
	min: (...Vector2) -> Vector2
	max: (...Vector2) -> Vector2
end

declare extern type Vector2int16 with
	new: (x: number?, y: number?) -> Vector2int16
end

declare extern type Vector3 with
	fromNormalId: (normal: Enum.NormalId) -> Vector3
	fromAxis: (axis: Enum.Axis) -> Vector3
	min: (...Vector3) -> Vector3
	max: (...Vector3) -> Vector3
end

declare extern type Vector3int16 with
	new: (x: number?, y: number?, z: number?) -> Vector3int16
end

declare extern type UDim with
	new: (scale: number?, offset: number?) -> UDim
end

declare extern type UDim2 with
	new: ((xScale: number?, xOffset: number?, yScale: number?, yOffset: number?) -> UDim2)
		& ((x: UDim, y: UDim?) -> UDim2)
end

declare extern type Random with
	new: (seed: number?) -> Random
end

declare extern type CFrame with
	fromMatrix: ((position: Vector3, right: Vector3, up: Vector3) -> CFrame)
		& ((position: Vector3, right: Vector3, up: Vector3, back: Vector3) -> CFrame)
	lookAt: ((position: Vector3, target: Vector3) -> CFrame)
		& ((position: Vector3, target: Vector3, up: Vector3) -> CFrame)
	lookAlong: ((position: Vector3, direction: Vector3) -> CFrame)
		& ((position: Vector3, direction: Vector3, up: Vector3) -> CFrame)
	fromEulerAngles: ((x: number, y: number, z: number) -> CFrame)
		& ((x: number, y: number, z: number, order: Enum.RotationOrder) -> CFrame)
end

declare extern type Font with
	new: ((family: string) -> Font)
		& ((family: string, weight: Enum.FontWeight) -> Font)
		& ((family: string, weight: Enum.FontWeight, style: Enum.FontStyle) -> Font)
	fromName: ((name: string) -> Font)
		& ((name: string, weight: Enum.FontWeight) -> Font)
		& ((name: string, weight: Enum.FontWeight, style: Enum.FontStyle) -> Font)
	fromId: ((identifier: number) -> Font)
		& ((identifier: number, weight: Enum.FontWeight) -> Font)
		& ((identifier: number, weight: Enum.FontWeight, style: Enum.FontStyle) -> Font)
end

declare extern type NumberSequence with
	new: ((start: number, finish: number?) -> NumberSequence)
		& ((keypoints: { NumberSequenceKeypoint }) -> NumberSequence)
end

declare extern type ColorSequence with
	new: ((start: Color3, finish: Color3?) -> ColorSequence) & ((keypoints: { ColorSequenceKeypoint }) -> ColorSequence)
end

declare extern type NumberSequenceKeypoint with
	new: (time: number, value: number, envelope: number?) -> NumberSequenceKeypoint
end

declare extern type User with
	fromId: ((identifier: number) -> User)
		& ((identifier: number, domain: Enum.DomainType, domainIdentifier: number) -> User)
end

declare extern type SharedTable with
	new: (() -> SharedTable) & ((value: { [any]: any }) -> SharedTable)
	clone: ((value: SharedTable) -> SharedTable) & ((value: SharedTable, deep: boolean) -> SharedTable)
	cloneAndFreeze: ((value: SharedTable) -> SharedTable) & ((value: SharedTable, deep: boolean) -> SharedTable)
	update: (value: SharedTable, key: string | number, callback: (any) -> any) -> ()
end

declare extern type PathWaypoint with
	new: (() -> PathWaypoint) & ((position: Vector3, action: Enum.PathWaypointAction?, label: string?) -> PathWaypoint)
end

declare extern type DateTime with
	fromIsoDate: (() -> nil) & ((value: string) -> DateTime?)
	fromUnixTimestamp: (() -> DateTime) & ((seconds: number) -> DateTime)
	fromUnixTimestampMillis: (() -> DateTime) & ((milliseconds: number) -> DateTime)
	fromUniversalTime: (() -> DateTime)
		& ((year: number) -> DateTime)
		& ((year: number, month: number) -> DateTime)
		& ((year: number, month: number, day: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number, minute: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number, minute: number, second: number) -> DateTime)
		& (
			(
				year: number,
				month: number,
				day: number,
				hour: number,
				minute: number,
				second: number,
				millisecond: number
			) -> DateTime
		)
	fromLocalTime: (() -> DateTime)
		& ((year: number) -> DateTime)
		& ((year: number, month: number) -> DateTime)
		& ((year: number, month: number, day: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number, minute: number) -> DateTime)
		& ((year: number, month: number, day: number, hour: number, minute: number, second: number) -> DateTime)
		& (
			(
				year: number,
				month: number,
				day: number,
				hour: number,
				minute: number,
				second: number,
				millisecond: number
			) -> DateTime
		)
end

declare extern type DockWidgetPluginGuiInfo with
	new: (() -> DockWidgetPluginGuiInfo)
		& ((state: Enum.InitialDockState) -> DockWidgetPluginGuiInfo)
		& ((state: Enum.InitialDockState, enabled: boolean) -> DockWidgetPluginGuiInfo)
		& ((state: Enum.InitialDockState, enabled: boolean, override: boolean) -> DockWidgetPluginGuiInfo)
		& (
			(
				state: Enum.InitialDockState,
				enabled: boolean,
				override: boolean,
				width: number
			) -> DockWidgetPluginGuiInfo
		)
		& (
			(
				state: Enum.InitialDockState,
				enabled: boolean,
				override: boolean,
				width: number,
				height: number
			) -> DockWidgetPluginGuiInfo
		)
		& (
			(
				state: Enum.InitialDockState,
				enabled: boolean,
				override: boolean,
				width: number,
				height: number,
				minimumWidth: number
			) -> DockWidgetPluginGuiInfo
		)
		& (
			(
				state: Enum.InitialDockState,
				enabled: boolean,
				override: boolean,
				width: number,
				height: number,
				minimumWidth: number,
				minimumHeight: number
			) -> DockWidgetPluginGuiInfo
		)
end

declare extern type TweenInfo with
	new: (() -> TweenInfo)
		& ((time: number) -> TweenInfo)
		& ((time: number, style: Enum.EasingStyle) -> TweenInfo)
		& ((time: number, style: Enum.EasingStyle, direction: Enum.EasingDirection) -> TweenInfo)
		& ((time: number, style: Enum.EasingStyle, direction: Enum.EasingDirection, repeatCount: number) -> TweenInfo)
		& (
			(
				time: number,
				style: Enum.EasingStyle,
				direction: Enum.EasingDirection,
				repeatCount: number,
				reverses: boolean
			) -> TweenInfo
		)
		& (
			(
				time: number,
				style: Enum.EasingStyle,
				direction: Enum.EasingDirection,
				repeatCount: number,
				reverses: boolean,
				delayTime: number
			) -> TweenInfo
		)
end

declare extern type NumberRange with
	new: (minimum: number, maximum: number?) -> NumberRange
end

declare extern type Ray with
	new: (() -> Ray) & ((origin: Vector3) -> Ray) & ((origin: Vector3, direction: Vector3) -> Ray)
end

declare extern type Region3 with
	new: (() -> Region3) & ((minimum: Vector3) -> Region3) & ((minimum: Vector3, maximum: Vector3) -> Region3)
end

declare extern type Region3int16 with
	new: (() -> Region3int16)
		& ((minimum: Vector3int16) -> Region3int16)
		& ((minimum: Vector3int16, maximum: Vector3int16) -> Region3int16)
end

declare extern type FloatCurveKey with
	new: ((time: number, value: number) -> FloatCurveKey)
		& ((time: number, value: number, interpolation: Enum.KeyInterpolationMode) -> FloatCurveKey)
end

declare extern type RotationCurveKey with
	new: ((time: number, value: CFrame) -> RotationCurveKey)
		& ((time: number, value: CFrame, interpolation: Enum.KeyInterpolationMode) -> RotationCurveKey)
end

declare extern type ValueCurveKey with
	new: ((time: number, value: any) -> ValueCurveKey)
		& ((time: number, value: any, interpolation: Enum.KeyInterpolationMode) -> ValueCurveKey)
end
