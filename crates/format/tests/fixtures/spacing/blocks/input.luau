local function choose(value:number):number if value>0 then return value elseif value==0 then return 1 else return -value end end
for index=1,3,1 do if index==2 then continue end emit(index) end
for key,value in entries do emit(key,value) end
while ready do step() break end
repeat step() until done
do finish() end

local count=1+2*3^2//4%5
local negative=1- -2
local ready=not false and count>=1 or count~=0
local length=#items
local label="left".."right"
count+=1
count-=2
count*=3
count/=4
count//=5
count%=6
count^=2
label..="!"

function object:read( index )
  if self.items [ index ] then
      return self.items [ index ].value
  end
  return nil
end
