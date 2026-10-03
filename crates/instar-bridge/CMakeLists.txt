cmake_minimum_required(VERSION 3.10)
project(Instar LANGUAGES CXX)

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)

set(LUAU_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/vendor/luau")
add_subdirectory(${LUAU_SOURCE_DIR} luau)

install(
    TARGETS
        Luau.Analysis
        Luau.Config
        Luau.Compiler
        Luau.Bytecode
        Luau.Ast
        Luau.VM
        Luau.Common
    ARCHIVE DESTINATION lib
)
