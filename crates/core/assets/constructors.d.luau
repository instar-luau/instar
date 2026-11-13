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
end

declare extern type Vector2 with
	min: (...Vector2) -> Vector2
	max: (...Vector2) -> Vector2
end

declare extern type Vector3 with
	fromNormalId: (normal: Enum.NormalId) -> Vector3
	fromAxis: (axis: Enum.Axis) -> Vector3
	min: (...Vector3) -> Vector3
	max: (...Vector3) -> Vector3
end

declare extern type SharedTable with
	new: (value: { [any]: any }?) -> SharedTable
	update: (value: SharedTable, key: string | number, callback: (any) -> any) -> ()
end
