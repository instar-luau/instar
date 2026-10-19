cmake_minimum_required(VERSION 3.15)
project(Instar LANGUAGES CXX C)

set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/lib>")
add_subdirectory(vendor/luau luau)
add_custom_target(instar DEPENDS Luau.Analysis Luau.Require Luau.CLI.lib)
