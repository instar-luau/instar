local reference = script.Parent.Reference
local module = reference.Value
require(module)
require(reference["Value"])
require(script.Parent.Model.PrimaryPart)
require(script.Parent.Actor.PrimaryPart)
require(script.Parent.Actor.Name)
require(reference:FindFirstChild("Value"))
require(script.Parent.Folder.Value)
