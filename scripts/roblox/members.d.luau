declare extern type Vector3 with
	function Angle(self, other: Vector3, axis: Vector3?): number
	function __add(self, other: Vector3): Vector3
	function __sub(self, other: Vector3): Vector3
	function __mul(self, other: Vector3 | number): Vector3
	function __div(self, other: Vector3 | number): Vector3
	function __unm(self): Vector3
	function __idiv(self, other: Vector3 | number): Vector3
end

declare extern type Vector2 with
	function FuzzyEq(self, other: Vector2, epsilon: number?): boolean
	function __add(self, other: Vector2): Vector2
	function __sub(self, other: Vector2): Vector2
	function __mul(self, other: Vector2 | number): Vector2
	function __div(self, other: Vector2 | number): Vector2
	function __idiv(self, other: Vector2 | number): Vector2
	function __unm(self): Vector2
end

declare extern type Vector3int16 with
	function __add(self, other: Vector3int16): Vector3int16
	function __sub(self, other: Vector3int16): Vector3int16
	function __mul(self, other: Vector3int16 | number): Vector3int16
	function __div(self, other: Vector3int16 | number): Vector3int16
	function __unm(self): Vector3int16
end

declare extern type Vector2int16 with
	function __add(self, other: Vector2int16): Vector2int16
	function __sub(self, other: Vector2int16): Vector2int16
	function __mul(self, other: Vector2int16 | number): Vector2int16
	function __div(self, other: Vector2int16 | number): Vector2int16
	function __unm(self): Vector2int16
end

declare extern type UDim2 with
	function __add(self, other: UDim2): UDim2
	function __sub(self, other: UDim2): UDim2
	function __unm(self): UDim2
end

declare extern type UDim with
	function __add(self, other: UDim): UDim
	function __sub(self, other: UDim): UDim
	function __unm(self): UDim
end

declare extern type CFrame with
	function ToWorldSpace(self, ...: CFrame): ...CFrame
	function ToObjectSpace(self, ...: CFrame): ...CFrame
	function PointToWorldSpace(self, ...: Vector3): ...Vector3
	function PointToObjectSpace(self, ...: Vector3): ...Vector3
	function VectorToWorldSpace(self, ...: Vector3): ...Vector3
	function VectorToObjectSpace(self, ...: Vector3): ...Vector3
	function __add(self, other: Vector3): CFrame
	function __sub(self, other: Vector3): CFrame
	function __mul(self, other: CFrame): CFrame
	function __mul(self, other: Vector3): Vector3
end

declare extern type Random with
	function Shuffle(self, table: { any })
end

declare extern type UserSettings with
	GameSettings: UserGameSettings
	function GetService(self, service: "UserGameSettings"): UserGameSettings
end

declare extern type Object with
	function GetPropertyChangedSignal(self, property: string): RBXScriptSignal<>
end

declare extern type Instance with
	Parent: Instance?
	AncestryChanged: RBXScriptSignal<Instance, Instance?>
	function FindFirstAncestor(self, name: string): Instance?
	function FindFirstAncestorOfClass(self, className: string): Instance?
	function FindFirstAncestorWhichIsA(self, className: string): Instance?
	function FindFirstChild(self, name: string, recursive: boolean?): Instance?
	function FindFirstChildOfClass(self, className: string): Instance?
	function FindFirstChildWhichIsA(self, className: string, recursive: boolean?): Instance?
	function FindFirstDescendant(self, name: string): Instance?
	function GetActor(self): Actor?
	function WaitForChild(self, name: string): Instance
	function WaitForChild(self, name: string, timeout: number): Instance?
	function GetAttribute(self, attribute: string): unknown?
	function GetAttributes(self): { [string]: unknown }
	function GetAttributeChangedSignal(self, attribute: string): RBXScriptSignal<>
end

declare extern type Model with
	PrimaryPart: BasePart?
end

declare extern type RemoteEvent with
	function FireAllClients(self, ...: any): ()
	function FireClient(self, player: Player, ...: any): ()
	function FireServer(self, ...: any): ()
	OnClientEvent: RBXScriptSignal<...any>
	OnServerEvent: RBXScriptSignal<(Player, ...any)>
end

declare extern type UnreliableRemoteEvent with
	function FireAllClients(self, ...: any): ()
	function FireClient(self, player: Player, ...: any): ()
	function FireServer(self, ...: any): ()
	OnClientEvent: RBXScriptSignal<...any>
	OnServerEvent: RBXScriptSignal<(Player, ...any)>
end

declare extern type RemoteFunction with
	function InvokeClient(self, player: Player, ...: any): ...any
	function InvokeServer(self, ...: any): ...any
	OnClientInvoke: (...any) -> ...any
	OnServerInvoke: (player: Player, ...any) -> ...any
end

declare extern type BindableEvent with
	function Fire(self, ...: any): ()
	Event: RBXScriptSignal<...any>
end

declare extern type BindableFunction with
	function Invoke(self, ...: any): ...any
	OnInvoke: (...any) -> ...any
end

declare extern type Players with
	PlayerChatted: RBXScriptSignal<EnumPlayerChatType, Player, string, Player?>
	function GetPlayerByUserId(self, userId: number): Player?
	function GetPlayerFromCharacter(self, character: Model): Player?
end

declare extern type ContextActionService with
	function BindAction(
		self,
		actionName: string,
		functionToBind: (actionName: string, inputState: EnumUserInputState, inputObject: InputObject) -> EnumContextActionResult?,
		createTouchButton: boolean,
		...: EnumUserInputType | EnumKeyCode
	): ()
	function BindActionAtPriority(
		self,
		actionName: string,
		functionToBind: (actionName: string, inputState: EnumUserInputState, inputObject: InputObject) -> EnumContextActionResult?,
		createTouchButton: boolean,
		priorityLevel: number,
		...: EnumUserInputType | EnumKeyCode
	): ()
end

declare extern type Plugin with
	function OpenScript(self, script: LuaSourceContainer, lineNumber: number?): nil
end

declare extern type PluginToolbar with
	function CreateButton(self, id: string, toolTip: string, iconAsset: string, text: string?): PluginToolbarButton
end

declare extern type WorldRoot with
	function Raycast(self, origin: Vector3, direction: Vector3, raycastParams: RaycastParams?): RaycastResult?
	function ArePartsTouchingOthers(self, partList: { BasePart }, overlapIgnored: number?): boolean
	function BulkMoveTo(self, partList: { BasePart }, cframeList: { CFrame }, eventMode: EnumBulkMoveMode?): nil
	function GetPartBoundsInBox(self, cframe: CFrame, size: Vector3, overlapParams: OverlapParams?): { BasePart }
	function GetPartBoundsInRadius(self, position: Vector3, radius: number, overlapParams: OverlapParams?): { BasePart }
	function GetPartsInPart(self, part: BasePart, overlapParams: OverlapParams?): { BasePart }
end

declare extern type HttpService with
	function RequestAsync(self, options: HttpRequestOptions): HttpResponseData
end

declare extern type HumanoidDescription with
	function GetAccessories(self, includeRigidAccessories: boolean): { HumanoidDescriptionAccessory }
	function SetAccessories(self, accessories: { HumanoidDescriptionAccessory }, includeRigidAccessories: boolean): ()
	function GetEmotes(self): { [string]: { number } }
	function SetEmotes(self, emotes: { [string]: { number } }): ()
	function GetEquippedEmotes(self): { { Slot: number, Name: string } }
	function SetEquippedEmotes(self, equippedEmotes: { string } | { Slot: number, Name: string }): ()
end

declare extern type TeleportService with
	function GetLocalPlayerTeleportData(self): TeleportData?
	function GetPlayerPlaceInstanceAsync(self, userId: number): (boolean, string, number, string)
	function Teleport(
		self,
		placeId: number,
		player: Player?,
		teleportData: TeleportData?,
		customLoadingScreen: GuiObject?
	)
	function TeleportAsync(
		self,
		placeId: number,
		players: { Player },
		teleportOptions: TeleportOptions?
	): TeleportAsyncResult
	function TeleportPartyAsync(
		self,
		placeId: number,
		players: { Player },
		teleportData: TeleportData?,
		customLoadingScreen: GuiObject?
	): string
	function TeleportToPlaceInstance(
		self,
		placeId: number,
		instanceId: string,
		player: Player?,
		spawnName: string?,
		teleportData: TeleportData?,
		customLoadingScreen: GuiObject?
	)
	function TeleportToPrivateServer(
		self,
		placeId: number,
		reservedServerAccessCode: string,
		players: { Player },
		spawnName: string?,
		teleportData: TeleportData?,
		customLoadingScreen: GuiObject?
	): nil
	function TeleportToSpawnByName(
		self,
		placeId: number,
		spawnName: string,
		player: Player?,
		teleportData: TeleportData?,
		customLoadingScreen: GuiObject?
	)
	function ReserveServer(self, placeId: number): (string, string)
	LocalPlayerArrivedFromTeleport: RBXScriptSignal<Player, any>
	TeleportInitFailed: RBXScriptSignal<Player, EnumTeleportResult, string, number, TeleportOptions>
end

declare extern type TeleportOptions with
	function GetTeleportData(self): TeleportData?
	function SetTeleportData(self, teleportData: TeleportData)
end

declare extern type UserService with
	function GetUserInfosByUserIdsAsync(
		self,
		userIds: { number }
	): { { Id: number, Username: string, DisplayName: string } }
end

declare extern type Studio with
	Theme: StudioTheme
end

declare extern type BasePlayerGui with
	function GetGuiObjectsAtPosition(self, x: number, y: number): { GuiObject }
	function GetGuiObjectsInCircle(self, position: Vector2, radius: number): { GuiObject }
end

declare extern type Path with
	function GetWaypoints(self): { PathWaypoint }
end

declare extern type CollectionService with
	function GetAllTags(self): { string }
	function GetTags(self, instance: Instance): { string }
	function GetInstanceAddedSignal(self, tag: string): RBXScriptSignal<Instance>
	function GetInstanceRemovedSignal(self, tag: string): RBXScriptSignal<Instance>
end

declare extern type UserInputService with
	function GetConnectedGamepads(self): { EnumUserInputType }
	function GetGamepadState(self, gamepadNum: EnumUserInputType): { InputObject }
	function GetKeysPressed(self): { InputObject }
	function GetMouseButtonsPressed(self): { InputObject }
	function GetNavigationGamepads(self): { EnumUserInputType }
	function GetSupportedGamepadKeyCodes(self, gamepadNum: EnumUserInputType): { EnumKeyCode }
end

declare extern type Humanoid with
	RootPart: BasePart?
	SeatPart: Seat | VehicleSeat | nil
	WalkToPart: BasePart?
	function GetAccessories(self): { Accessory }
end

declare extern type Player with
	Character: Model?
	Chatted: RBXScriptSignal<string, Player?>
	ReplicationFocus: Instance?
	function GetJoinData(self): {
		LaunchData: string?,
		Members: { number }?,
		SourceGameId: number?,
		SourcePlaceId: number?,
		TeleportData: TeleportData?,
	}
end

declare extern type InstanceAdornment with
	Adornee: Instance?
end

declare extern type BasePart with
	function GetConnectedParts(self, recursive: boolean?): { BasePart }
	function GetNetworkOwner(self): Player?
	function GetTouchingParts(self): { BasePart }
	function SubtractAsync(
		self,
		parts: { BasePart },
		collisionfidelity: EnumCollisionFidelity?,
		renderFidelity: EnumRenderFidelity?
	): UnionOperation
	function UnionAsync(
		self,
		parts: { BasePart },
		collisionfidelity: EnumCollisionFidelity?,
		renderFidelity: EnumRenderFidelity?
	): UnionOperation
end

declare extern type Team with
	function GetPlayers(self): { Player }
end

declare extern type Teams with
	function GetTeams(self): { Team }
end

declare extern type Camera with
	CameraSubject: Humanoid | BasePart | nil
	function GetPartsObscuringTarget(self, castPoints: { Vector3 }, ignoreList: { Instance }): { BasePart }
end

declare extern type RunService with
	function BindToRenderStep(self, name: string, priority: number, func: ((delta: number) -> ())): ()
end

declare extern type GuiService with
	SelectedObject: GuiObject?
end

declare extern type TextChatService with
	ChatWindowConfiguration: ChatWindowConfiguration
	ChatInputBarConfiguration: ChatInputBarConfiguration
	BubbleChatConfiguration: BubbleChatConfiguration
	ChannelTabsConfiguration: ChannelTabsConfiguration
end

declare extern type GlobalDataStore with
	function GetAsync(self, key: string, options: DataStoreGetOptions?): (any, DataStoreKeyInfo)
	function IncrementAsync(
		self,
		key: string,
		delta: number?,
		userIds: { number }?,
		options: DataStoreIncrementOptions?
	): (number, DataStoreKeyInfo)
	function RemoveAsync(self, key: string): (any, DataStoreKeyInfo)
	function SetAsync(self, key: string, value: any, userIds: { number }?, options: DataStoreSetOptions?): string
	function UpdateAsync(
		self,
		key: string,
		transformFunction: ((any, DataStoreKeyInfo) -> (any, { number }?, {}?))
	): (any, DataStoreKeyInfo)
end

declare extern type OrderedDataStore with
	function GetAsync(self, key: string, options: DataStoreGetOptions?): (number?, DataStoreKeyInfo)
	function GetSortedAsync(
		self,
		ascending: boolean,
		pageSize: number,
		minValue: number?,
		maxValue: number?
	): DataStorePages
	function RemoveAsync(self, key: string): (number?, DataStoreKeyInfo)
	function SetAsync(self, key: string, value: number, userIds: { number }?, options: DataStoreSetOptions?): string
	function UpdateAsync(
		self,
		key: string,
		transformFunction: ((number?, DataStoreKeyInfo) -> (number, { number }?, {}?))
	): (number?, DataStoreKeyInfo)
end

declare extern type Highlight with
	Adornee: Instance?
end

declare extern type PartAdornment with
	Adornee: BasePart?
end

declare extern type JointInstance with
	Part0: BasePart?
	Part1: BasePart?
end

declare extern type ObjectValue with
	Value: Instance?
	Changed: RBXScriptSignal<Instance?>
end

declare extern type Actor with
	function SendMessage(self, topic: string, ...: any): ()
end

declare extern type Seat with
	Occupant: Humanoid?
end

declare extern type VehicleSeat with
	Occupant: Humanoid?
end

declare extern type Beam with
	Attachment0: Attachment?
	Attachment1: Attachment?
end

declare extern type Trail with
	Attachment0: Attachment?
	Attachment1: Attachment?
end

declare extern type Constraint with
	Attachment0: Attachment?
	Attachment1: Attachment?
end

declare extern type PathfindingLink with
	Attachment0: Attachment?
	Attachment1: Attachment?
end

declare extern type ControllerPartSensor with
	SensedPart: BasePart?
end

declare extern type Sound with
	SoundGroup: SoundGroup?
end

declare extern type StudioService with
	function GizmoRaycast(
		self,
		origin: Vector3,
		direction: Vector3,
		raycastParams: RaycastParams?
	): RaycastResult<Attachment | Constraint | NoCollisionConstraint | WeldConstraint>?
end

declare extern type ControllerManager with
	ActiveController: ControllerBase?
	ClimbSensor: ControllerSensor?
	GroundSensor: ControllerSensor?
	RootPart: BasePart?
end

declare extern type CaptureService with
	function StartVideoCaptureAsync(
		self,
		onCaptureReady: (capture: VideoCapture) -> (),
		params: CaptureParams
	): EnumVideoCaptureStartedResult
	function TakeCapture(self, onCaptureReady: (capture: Capture) -> (), params: CaptureParams): ()
end

declare extern type ModerationService with
	function BindReviewableContentEventProcessor(
		self,
		priority: number,
		callback: (event: ReviewableContentEvent) -> ()
	): RBXScriptConnection
end

declare extern type VideoSampler with
	function GetSamplesAtTimesAsync(self, times: { number }): { VideoSample }
end

declare extern type AvatarCreationService with
	function AutoSetupAvatarAsync(
		self,
		player: Player,
		model: Model,
		progressCallback: (progressInfo: { Progress: number }) -> ()?
	): string
	function AutoSetupAvatarNewAsync(
		self,
		player: Player,
		autoSetupParams: AutoSetupParams,
		progressCallback: (progressInfo: { Progress: number }) -> ()?
	): string
end

declare extern type AudioEmitter with
	function GetAngleAttenuation(self): { [number]: number }
	function GetDistanceAttenuation(self): { [number]: number }
	function SetAngleAttenuation(self, curve: { [number]: number }): nil
	function SetDistanceAttenuation(self, curve: { [number]: number }): nil
end

declare extern type AudioListener with
	function GetAngleAttenuation(self): { [number]: number }
	function GetDistanceAttenuation(self): { [number]: number }
	function SetAngleAttenuation(self, curve: { [number]: number }): nil
	function SetDistanceAttenuation(self, curve: { [number]: number }): nil
end

declare extern type ColorSequence with
	Keypoints: { ColorSequenceKeypoint }
end

declare extern type NumberSequence with
	Keypoints: { NumberSequenceKeypoint }
end

declare extern type RaycastParams with
	ExcludeInstances: { Instance }?
	IncludeInstances: { Instance }?
end

declare extern type OverlapParams with
	ExcludeInstances: { Instance }?
	IncludeInstances: { Instance }?
end
