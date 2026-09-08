cmake_minimum_required(VERSION 3.15)
project(instar-upstream-oracle LANGUAGES CXX)

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_WEB OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

set(LUAU_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../vendor/luau")
add_subdirectory("${LUAU_ROOT}" luau EXCLUDE_FROM_ALL)

add_executable(instar-upstream-oracle oracle.cpp)
target_compile_features(instar-upstream-oracle PRIVATE cxx_std_17)
target_link_libraries(instar-upstream-oracle PRIVATE Luau.Analysis Luau.Require Luau.CLI.lib)

set(GENERATED_FIXTURE "${CMAKE_CURRENT_BINARY_DIR}/Fixture.cpp")
set(GENERATED_PARSER_TEST "${CMAKE_CURRENT_BINARY_DIR}/Parser.test.cpp")
set(GENERATED_LEXER_TEST "${CMAKE_CURRENT_BINARY_DIR}/Lexer.test.cpp")
foreach(PAIR
    "${LUAU_ROOT}/tests/Fixture.cpp|${GENERATED_FIXTURE}"
    "${LUAU_ROOT}/tests/Parser.test.cpp|${GENERATED_PARSER_TEST}"
    "${LUAU_ROOT}/tests/Lexer.test.cpp|${GENERATED_LEXER_TEST}"
)
    string(REPLACE "|" ";" PATHS "${PAIR}")
    list(GET PATHS 0 INPUT)
    list(GET PATHS 1 OUTPUT)
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${INPUT}")
    file(READ "${INPUT}" CONTENT)
    string(REPLACE "Parser::parse(" "instarOracleParse(" CONTENT "${CONTENT}")
    string(REPLACE "Lexer lexer(" "InstarRecordingLexer lexer(" CONTENT "${CONTENT}")
    string(PREPEND CONTENT "#include \"recorder.h\"\n")
    file(WRITE "${OUTPUT}" "${CONTENT}")
endforeach()

add_executable(instar-upstream-parser-tests
    "${GENERATED_FIXTURE}"
    "${GENERATED_PARSER_TEST}"
    "${GENERATED_LEXER_TEST}"
    "${LUAU_ROOT}/tests/AstQueryDsl.cpp"
    "${LUAU_ROOT}/tests/RegisterCallbacks.cpp"
    "${LUAU_ROOT}/tests/main.cpp"
    recorder.cpp
)
target_compile_features(instar-upstream-parser-tests PRIVATE cxx_std_17)
if(MSVC)
    target_compile_options(instar-upstream-parser-tests PRIVATE /EHsc)
else()
    target_compile_options(instar-upstream-parser-tests PRIVATE -fexceptions)
endif()
target_compile_definitions(instar-upstream-parser-tests PRIVATE DOCTEST_CONFIG_DOUBLE_STRINGIFY DOCTEST_CONFIG_USE_STD_HEADERS)
target_include_directories(instar-upstream-parser-tests PRIVATE
    "${CMAKE_CURRENT_LIST_DIR}"
    "${LUAU_ROOT}/tests"
    "${LUAU_ROOT}/extern"
)
target_link_libraries(instar-upstream-parser-tests PRIVATE Luau.Analysis Luau.Bytecode Luau.Compiler Luau.CodeGen)

install(TARGETS instar-upstream-oracle instar-upstream-parser-tests RUNTIME DESTINATION bin)
