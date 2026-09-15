cmake_minimum_required(VERSION 3.10)
project(Instar LANGUAGES CXX C)

add_subdirectory(../../../vendor/luau luau EXCLUDE_FROM_ALL)
add_library(Instar STATIC engine.cpp)
target_link_libraries(Instar PRIVATE Luau.Analysis Luau.CLI.lib)
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
    DESTINATION lib
)
