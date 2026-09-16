cmake_minimum_required(VERSION 3.10)
project(Instar LANGUAGES CXX)

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)

add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../../../vendor/luau luau)

add_library(Instar STATIC bridge.cpp)
target_link_libraries(Instar PRIVATE Luau.Analysis Luau.Config Luau.Compiler Luau.VM)

install(
    TARGETS
        Instar
        Luau.Analysis
        Luau.CLI.lib
        Luau.Config
        Luau.Compiler
        Luau.Bytecode
        Luau.Ast
        Luau.VM
        Luau.Common
    ARCHIVE DESTINATION lib
)
