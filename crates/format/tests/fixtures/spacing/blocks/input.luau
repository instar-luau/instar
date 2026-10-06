local function choose(value:number):number if value>0 then return value elseif value==0 then return 1 else return -value end end
for index=1,3,1 do if index==2 then continue end emit(index) end
for key,value in entries do emit(key,value) end
while ready do step() break end
repeat step() until done
do finish() end
