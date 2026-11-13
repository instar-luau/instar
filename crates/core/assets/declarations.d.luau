type ContentId = string
type ProtectedString = string
type BinaryString = string
type QDir = string
type QFont = string
type TeleportData = boolean | buffer | number | string | { [number]: TeleportData } | { [string]: TeleportData }

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
	Url: string | Secret,
	Method: "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "TRACE" | "PATCH" | nil,
	Headers: { [string]: string | Secret }?,
	Body: string?,
	Compress: Enum.HttpCompression?,
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
	AccessoryType: Enum.AccessoryType,
	IsLayered: boolean,
	Order: number?,
	Puffiness: number?,
}

declare CatalogSearchParams: {
	new: () -> CatalogSearchParams,
}

export type RaycastResult<T = BasePart> = {
	Instance: T,
	Position: Vector3,
	Normal: Vector3,
	Material: Enum.Material,
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

@[deprecated { use = "plugin" }]
declare function PluginManager(): PluginManager

@[deprecated { use = 'game:GetService("DebuggerManager")' }]
declare function DebuggerManager(): DebuggerManager

@[deprecated { use = "task.delay" }]
declare function delay(delay: number?, callback: (dt: number, gt: number) -> ())

@[deprecated { use = "gcinfo" }]
declare function collectgarbage(mode: "count"): number

@[deprecated { use = 'game:GetService("Stats")' }]
declare function stats(): Stats

@[deprecated { use = "task.wait" }]
declare function wait(delay: number?): (number, number)

@[deprecated { use = "task.delay" }]
declare function Delay(delay: number?, callback: (dt: number, gt: number) -> ())

@[deprecated { use = "task.spawn" }]
declare function Spawn(callback: (dt: number, gt: number) -> ())

@[deprecated { use = "task.wait" }]
declare function Wait(delay: number?): (number, number)

@[deprecated { use = "elapsedTime" }]
declare function ElapsedTime(): number

@[deprecated { use = 'game:GetService("Stats")' }]
declare function Stats(): Stats

@[deprecated { use = "version" }]
declare function Version(): string

type ReflectedClassOrNil = ReflectedClass?
type ReflectedClasses = { ReflectedClass }
type ReflectedProperties = { ReflectedProperty }
type ProductIdentifierArray = { ProductIdentifier }
type InfoTypeArray = { Enum.InfoType }
type RankedItemArray = { RankedItem }

declare extern type DockWidgetPluginGuiInfo with
end
declare extern type ClipEvaluator with
end
declare extern type OpenCloudModel with
end
declare extern type ScopedInstanceIdentity with
end
declare extern type SystemAddress with
end
declare extern type UniqueId with
end
declare extern type WebViewParams with
end

declare extern type Collection with
	Enabled: boolean
	OnAdded: (instance: Instance) -> ()
	OnRemoved: (instance: Instance) -> ()
	OnHeartbeat: (instance: Instance, deltaTime: number) -> ()
	OnPreRender: (instance: Instance, deltaTime: number) -> ()
	OnPreAnimation: (instance: Instance, deltaTime: number) -> ()
	OnPreSimulation: (instance: Instance, deltaTime: number) -> ()
	OnPostSimulation: (instance: Instance, deltaTime: number) -> ()
	OnSimulate: (instance: Instance, deltaTime: number) -> ()
	OnPropertyChanged: { [string]: (instance: Instance) -> () }
	OnAttributeChanged: { [string]: (instance: Instance) -> () }
	function ForEach(self, callback: (instance: Instance) -> ()): ()
	function Destroy(self): ()
end

type CollectionHandle = Collection
