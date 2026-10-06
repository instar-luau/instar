cmake_minimum_required(VERSION 3.15)
project(instar LANGUAGES CXX C)

if(MSVC)
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:/EHsc>")
else()
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:-fexceptions>")
endif()

add_subdirectory(vendor/luau)
target_compile_definitions(Luau.VM PRIVATE LUA_USE_LONGJMP=1)

add_library(configuration STATIC src/configuration.cpp "${CXX_BRIDGE_SOURCE}")
target_include_directories(configuration PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}" "${CXX_INCLUDE_DIRECTORY}" "${CXX_CRATE_DIRECTORY}")
target_link_libraries(configuration PRIVATE Luau.Config Luau.VM)
