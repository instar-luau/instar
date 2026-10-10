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
    native/engine.cpp
    native/parsing.cpp
    native/environment.cpp
    native/analysis.cpp
    native/semantics.cpp
    native/linting.cpp
    native/completion.cpp
    "${CXX_BRIDGE_SOURCE}"
)
target_include_directories(bridge PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}" "${CXX_INCLUDE_DIRECTORY}" "${CXX_CRATE_DIRECTORY}")
target_link_libraries(bridge PRIVATE Luau.Config Luau.Compiler Luau.VM Luau.Analysis)
