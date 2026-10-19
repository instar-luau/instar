while active do
    if ready then
        work()
    else
        wait_for_work()
    end
end
