local function collect(items)
  for index, value in items do
    if value then
      emit(index, value)
    end
  end
end
