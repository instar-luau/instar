declare extern type Instance with
    read Name: string
end
declare extern type DataModel extends Instance with end
declare extern type Part extends Instance with end
declare Instance: {new: (name: string) -> Instance}
declare game: DataModel
declare script: Instance

declare extern type PVInstance extends Instance with end
declare extern type Model extends PVInstance with PrimaryPart: Part? end

declare extern type Folder extends Instance with end
declare extern type LuaSourceContainer extends Instance with end
declare extern type BaseScript extends LuaSourceContainer with end
declare extern type Script extends BaseScript with end
declare extern type ModuleScript extends LuaSourceContainer with end
declare extern type ReplicatedStorage extends Instance with end
declare extern type WorldRoot extends Model with end
declare extern type Workspace extends WorldRoot with end