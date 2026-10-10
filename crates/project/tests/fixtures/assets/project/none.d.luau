declare extern type Instance with
    read Name: string
end
declare extern type DataModel extends Instance with end
declare extern type Part extends Instance with end
declare extern type ObjectValue extends Instance with
    Value: Instance?
end
declare extern type Model extends Instance with
    PrimaryPart: Part?
end
declare extern type Actor extends Model with end
declare Instance: {new: (name: string) -> Instance}
declare game: DataModel
declare script: Instance