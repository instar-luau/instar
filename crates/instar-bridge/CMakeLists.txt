cmake_minimum_required(VERSION 3.10)
project(Instar LANGUAGES CXX)

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)

set(LUAU_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/vendor/luau")
add_subdirectory(${LUAU_SOURCE_DIR} luau)

add_library(Instar STATIC native/bridge.cpp native/editor.cpp native/roblox.cpp)
target_compile_features(Instar PRIVATE cxx_std_17)
target_include_directories(
    Instar
    PRIVATE
        ${LUAU_SOURCE_DIR}/Common/include
        ${LUAU_SOURCE_DIR}/Ast/include
        ${LUAU_SOURCE_DIR}/Bytecode/include
        ${LUAU_SOURCE_DIR}/Compiler/include
        ${LUAU_SOURCE_DIR}/Config/include
        ${LUAU_SOURCE_DIR}/Analysis/include
        ${LUAU_SOURCE_DIR}/VM/include
)

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
