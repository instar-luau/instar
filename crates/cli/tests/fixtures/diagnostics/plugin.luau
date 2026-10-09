declare extern type Instance with
    read Name: string
end
declare extern type DataModel extends Instance with end
declare extern type Part extends Instance with end
declare Instance: {new: (name: string) -> Instance}
declare game: DataModel
declare script: { Parent: { Target: Instance } }
declare plugin: {Name: string}
