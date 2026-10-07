cmake_minimum_required(VERSION 3.15)
project(instar LANGUAGES CXX C)

if(MSVC)
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:/EHsc>")
else()
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:-fexceptions>")
endif()

add_subdirectory(vendor/luau)
target_compile_definitions(Luau.VM PRIVATE LUA_USE_LONGJMP=1)

add_library(bridge STATIC
    native/configuration.cpp
    native/flags.cpp
    native/frontend.cpp
    native/frontend/checking.cpp
    native/frontend/diagnostics.cpp
    native/frontend/documentation.cpp
    native/frontend/linting.cpp
    native/frontend/parsing.cpp
    native/frontend/roblox.cpp
    native/frontend/state.cpp
    "${CXX_BRIDGE_SOURCE}"
)
target_include_directories(bridge PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}" "${CXX_INCLUDE_DIRECTORY}" "${CXX_CRATE_DIRECTORY}")
target_link_libraries(bridge PRIVATE Luau.Config Luau.VM Luau.Analysis)
