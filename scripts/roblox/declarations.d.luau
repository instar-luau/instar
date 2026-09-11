type ContentId = string
type ProtectedString = string
type BinaryString = string
type QDir = string
type QFont = string
type TeleportData = boolean | buffer | number | string | { [number]: TeleportData } | { [string]: TeleportData }

declare extern type Enum with
	function GetEnumItems(self): { any }
	function FromValue(self, Number: number): any
	function FromName(self, Name: string): any
end

declare extern type EnumItem with
	Name: string
	Value: number
	EnumType: Enum
	function IsA(self, enumName: string): boolean
end

declare debug: {
	info: (<R...>(thread, number, string) -> R...)
		& (<R...>(number, string) -> R...)
		& (<A..., R1..., R2...>((A...) -> R1..., string) -> R2...),
	traceback: ((string?, number?) -> string) & ((thread, string?, number?) -> string),
	profilebegin: (label: string) -> (),
	profileend: () -> (),
	getmemorycategory: () -> string,
	setmemorycategory: (tag: string) -> (),
	resetmemorycategory: () -> (),
}

declare utf8: {
	char: (...number) -> string,
	charpattern: string,
	codepoint: (string, number?, number?) -> (...number),
	codes: (string) -> ((string, number) -> (number, number), string, number),
	graphemes: (string, number?, number?) -> (() -> (number, number)),
	len: (string, number?, number?) -> (number?, number?),
	nfcnormalize: (string) -> string,
	nfdnormalize: (string) -> string,
	offset: (string, number, number?) -> number?,
}

declare function warn<T...>(...: T...)

@[deprecated { use = "task.spawn" }]
declare function spawn(callback: (dt: number, gt: number) -> ())

declare extern type SharedTable with
	[string | number]: any
	function __iter(self): (any, number) -> (number, any)
end

export type RBXScriptSignal<T... = ...any> = {
	Wait: (self: RBXScriptSignal<T...>) -> T...,
	Connect: (self: RBXScriptSignal<T...>, callback: (T...) -> ()) -> RBXScriptConnection,
	ConnectParallel: (self: RBXScriptSignal<T...>, callback: (T...) -> ()) -> RBXScriptConnection,
	Once: (self: RBXScriptSignal<T...>, callback: (T...) -> ()) -> RBXScriptConnection,
}

type HttpRequestOptions = {
	Url: string,
	Method: "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "TRACE" | "PATCH" | nil,
	Headers: { [string]: string | Secret }?,
	Body: string?,
	Compress: EnumHttpCompression?,
}

type HttpResponseData = {
	Success: boolean,
	StatusCode: number,
	StatusMessage: string,
	Headers: { [string]: string },
	Body: string?,
}

type HumanoidDescriptionAccessory = {
	AssetId: number,
	AccessoryType: EnumAccessoryType,
	IsLayered: boolean,
	Order: number?,
	Puffiness: number?,
}

declare extern type ValueCurveKey with
	Interpolation: EnumKeyInterpolationMode
	Time: number
	Value: any
	RightTangent: number
	LeftTangent: number
end

declare ValueCurveKey: {
	new: (time: number, value: any, interpolation: EnumKeyInterpolationMode) -> ValueCurveKey,
}

export type RaycastResult<T = BasePart> = {
	Instance: T,
	Position: Vector3,
	Normal: Vector3,
	Material: EnumMaterial,
	Distance: number,
}

declare extern type GlobalSettings extends GenericSettings with
	Lua: LuaSettings
	Game: GameSettings
	Studio: Studio
	Network: NetworkSettings
	Physics: PhysicsSettings
	Rendering: RenderSettings
	Diagnostics: DebugSettings
	function GetFFlag(self, name: string): boolean
	function GetFVariable(self, name: string): string
end

declare SharedTable: {
	new: () -> SharedTable,
	new: (t: { [any]: any }?) -> SharedTable,
	clear: (st: SharedTable) -> (),
	clone: (st: SharedTable, deep: boolean?) -> SharedTable,
	cloneAndFreeze: (st: SharedTable, deep: boolean?) -> SharedTable,
	increment: (st: SharedTable, key: string | number, delta: number) -> number,
	isFrozen: (st: SharedTable) -> boolean,
	size: (st: SharedTable) -> number,
	update: (st: SharedTable, key: string | number, f: (any) -> any) -> (),
}

declare function settings(): GlobalSettings
declare function UserSettings(): UserSettings

@[deprecated {use = "plugin"}]
declare function PluginManager(): PluginManager

@[deprecated {use = 'game:GetService("DebuggerManager")'}]
declare function DebuggerManager(): DebuggerManager

@[deprecated{ use = "task.delay" }]
declare function delay(delay: number?, callback: (dt: number, gt: number) -> ())

@[deprecated{ use = "gcinfo" }]
declare function collectgarbage(mode: "count"): number

@[deprecated{ use = 'game:GetService("Stats")' }]
declare function stats(): Stats

@[deprecated{ use = "task.wait" }]
declare function wait(delay: number?): (number, number)

@deprecated
declare function printidentity(prefix: string?)

@deprecated
declare function version(): string

declare game: DataModel

declare workspace: Workspace

declare script: LuaSourceContainer

@[deprecated{ use = "task.delay" }]
declare function Delay(delay: number?, callback: (dt: number, gt: number) -> ())

@[deprecated{ use = "task.wait" }]
declare function Wait(delay: number?): (number, number)

@[deprecated{ use = "elapsedTime" }]
declare function ElapsedTime(): number

@[deprecated{ use = 'game:GetService("Stats")' }]
declare function Stats(): Stats

@[deprecated{ use = "version" }]
declare function Version(): string

type ReflectedClassOrNil = ReflectedClass?

type ReflectedClasses = { ReflectedClass }

type ReflectedProperties = { ReflectedProperty }
