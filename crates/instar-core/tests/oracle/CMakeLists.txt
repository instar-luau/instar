cmake_minimum_required(VERSION 3.15)
project(instar-oracle LANGUAGES CXX)

set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_WEB OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

set(LUAU_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../vendor/luau")
add_subdirectory("${LUAU_ROOT}" luau EXCLUDE_FROM_ALL)

add_executable(instar-oracle oracle.cpp recorder.cpp)
target_compile_features(instar-oracle PRIVATE cxx_std_17)
target_link_libraries(instar-oracle PRIVATE Luau.Analysis Luau.Require Luau.CLI.lib)

# Instrument implementation entry/exit paths, independent of test call spelling.
get_target_property(AST_SOURCES Luau.Ast SOURCES)
foreach(NAME Parser Lexer)
    set(INPUT "${LUAU_ROOT}/Ast/src/${NAME}.cpp")
    set(OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/${NAME}.cpp")
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${INPUT}")
    file(READ "${INPUT}" CONTENT)
    if(NAME STREQUAL "Parser")
        string(REGEX MATCHALL "Parser p\\(buffer, bufferSize, names, allocator, options\\)" ENTRIES "${CONTENT}")
        list(LENGTH ENTRIES ENTRY_COUNT)
        string(REGEX MATCHALL "return Parse(Result|NodeResult<Node>)\\{[^;]+\\}" RETURNS "${CONTENT}")
        list(LENGTH RETURNS RETURN_COUNT)
        if(NOT ENTRY_COUNT EQUAL 2 OR NOT RETURN_COUNT EQUAL 4)
            message(FATAL_ERROR "Parser instrumentation anchors changed: ${ENTRY_COUNT} entries, ${RETURN_COUNT} returns")
        endif()
        string(REPLACE "Parser p(buffer, bufferSize, names, allocator, options);" "const auto oracleInvocation = instarParseBegin();\n    Parser p(buffer, bufferSize, names, allocator, options);" CONTENT "${CONTENT}")
        string(REGEX REPLACE "return (Parse(Result|NodeResult<Node>)\\{[^;]+)\\};" "return instarRecordResult(oracleInvocation, buffer, bufferSize, options, \\1});" CONTENT "${CONTENT}")
    else()
        set(ANCHOR "    , readNames(true)\n{\n}")
        string(FIND "${CONTENT}" "${ANCHOR}" FOUND)
        if(FOUND EQUAL -1)
            message(FATAL_ERROR "Lexer instrumentation anchor changed")
        endif()
        string(REPLACE "${ANCHOR}" "    , readNames(true)\n{\n    instarRecordLex(buffer, bufferSize, startPosition);\n}" CONTENT "${CONTENT}")
    endif()
    string(PREPEND CONTENT "#include \"recorder.h\"\n")
    file(WRITE "${OUTPUT}" "${CONTENT}")
    list(FILTER AST_SOURCES EXCLUDE REGEX "(^|/)${NAME}\\.cpp$")
    list(APPEND AST_SOURCES "${OUTPUT}")
endforeach()
set_property(TARGET Luau.Ast PROPERTY SOURCES "${AST_SOURCES}")
target_include_directories(Luau.Ast PRIVATE "${CMAKE_CURRENT_LIST_DIR}")

# Keep the vendored checkout intact while filling its JSON visitor gaps.
set(ENCODER_INPUT "${LUAU_ROOT}/Analysis/src/AstJsonEncoder.cpp")
set(ENCODER_OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/AstJsonEncoder.cpp")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${ENCODER_INPUT}" "${CMAKE_CURRENT_LIST_DIR}/encoder.inc")
file(READ "${ENCODER_INPUT}" ENCODER)
string(PREPEND ENCODER "#include <cstdint>\n#include <cstring>\n")
set(NUMBER_VISITOR "    bool visit(class AstExprConstantNumber* node) override\n    {\n        write(node);")
string(FIND "${ENCODER}" "${NUMBER_VISITOR}" NUMBER_FOUND)
if(NUMBER_FOUND EQUAL -1)
    message(FATAL_ERROR "Number visitor instrumentation anchor changed")
endif()
string(REPLACE "${NUMBER_VISITOR}" "    bool visit(class AstExprConstantNumber* node) override\n    {\n        writeNumber(node);" ENCODER "${ENCODER}")
set(ENCODER_ANCHOR "    bool visit(class AstTypeGroup* node) override")
string(FIND "${ENCODER}" "${ENCODER_ANCHOR}" FOUND)
if(FOUND EQUAL -1)
    message(FATAL_ERROR "AST encoder instrumentation anchor changed")
endif()
string(REPLACE "${ENCODER_ANCHOR}" "#include \"encoder.inc\"\n${ENCODER_ANCHOR}" ENCODER "${ENCODER}")
# Preserve the node discriminator separately from declared-global type payloads.
string(REPLACE "PROP(type);" "write(\"luauType\", node->type);" ENCODER "${ENCODER}")
# JSON has no non-finite number tokens; retain them as explicit string values.
foreach(NUMBER Infinity -Infinity NaN)
    string(REPLACE "writeRaw(\"${NUMBER}\")" "writeRaw(\"\\\"${NUMBER}\\\"\")" ENCODER "${ENCODER}")
endforeach()
file(WRITE "${ENCODER_OUTPUT}" "${ENCODER}")
get_target_property(ANALYSIS_SOURCES Luau.Analysis SOURCES)
list(FILTER ANALYSIS_SOURCES EXCLUDE REGEX "(^|/)AstJsonEncoder\\.cpp$")
list(APPEND ANALYSIS_SOURCES "${ENCODER_OUTPUT}")
set_property(TARGET Luau.Analysis PROPERTY SOURCES "${ANALYSIS_SOURCES}")
target_include_directories(Luau.Analysis PRIVATE "${CMAKE_CURRENT_LIST_DIR}")

add_executable(instar-parser-tests
    "${LUAU_ROOT}/tests/Fixture.cpp"
    "${LUAU_ROOT}/tests/Parser.test.cpp"
    "${LUAU_ROOT}/tests/Lexer.test.cpp"
    "${LUAU_ROOT}/tests/AstQueryDsl.cpp"
    "${LUAU_ROOT}/tests/RegisterCallbacks.cpp"
    "${LUAU_ROOT}/tests/main.cpp"
    recorder.cpp
)
target_compile_features(instar-parser-tests PRIVATE cxx_std_17)
if(MSVC)
    target_compile_options(instar-parser-tests PRIVATE /EHsc)
else()
    target_compile_options(instar-parser-tests PRIVATE -fexceptions)
endif()
target_compile_definitions(instar-parser-tests PRIVATE DOCTEST_CONFIG_DOUBLE_STRINGIFY DOCTEST_CONFIG_USE_STD_HEADERS)
target_include_directories(instar-parser-tests PRIVATE
    "${CMAKE_CURRENT_LIST_DIR}"
    "${LUAU_ROOT}/tests"
    "${LUAU_ROOT}/extern"
)
target_link_libraries(instar-parser-tests PRIVATE Luau.Analysis Luau.Bytecode Luau.Compiler Luau.CodeGen)

install(TARGETS instar-oracle instar-parser-tests RUNTIME DESTINATION bin)
