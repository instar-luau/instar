consume((first_contribution + second_contribution + third_contribution))
consume((first_contribution ^ second_contribution ^ third_contribution))
consume((first_contribution .. second_contribution .. third_contribution))
consume((first_contribution and second_contribution and third_contribution))
consume((first_contribution or second_contribution or third_contribution))
type Union = (FirstContribution | SecondContribution | ThirdContribution)
type Intersection = (FirstContribution & SecondContribution & ThirdContribution)
local value = first_contribution .. second_contribution .. third_contribution
local power = first_contribution ^ second_contribution ^ third_contribution
if ((first_contribution and second_contribution and third_contribution)) then act() end
consume((first_contribution - (second_contribution - third_contribution)))
consume(((first_contribution ^ second_contribution) ^ third_contribution))
consume((first_contribution ^ (second_contribution ^ third_contribution)))
type Map = {[(FirstContribution | SecondContribution | ThirdContribution)]: boolean}
