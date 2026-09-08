#pragma once

#include "Luau/Ast.h"
#include "Luau/ParseOptions.h"
#include "Luau/ParseResult.h"

#include <cstdint>
#include <type_traits>

std::uint64_t instarParseBegin();
void instarRecordParsed(std::uint64_t invocation, const char *source,
                        size_t size, const Luau::ParseOptions &options,
                        Luau::AstNode *root,
                        const std::vector<Luau::ParseError> &errors,
                        const std::vector<Luau::Comment> &comments,
                        const char *entry);
void instarRecordLex(const char *source, size_t size, Luau::Position start);

template <typename Result>
Result instarRecordResult(std::uint64_t invocation, const char *source,
                          size_t size, const Luau::ParseOptions &options,
                          Result result) {
  using Node = std::remove_pointer_t<decltype(result.root)>;
  const char *entry = std::is_same_v<Node, Luau::AstStatBlock> ? "module"
                      : std::is_same_v<Node, Luau::AstExpr>    ? "expression"
                      : std::is_same_v<Node, Luau::AstType>    ? "type"
                                                               : "unknown";
  instarRecordParsed(invocation, source, size, options, result.root,
                     result.errors, result.commentLocations, entry);
  return result;
}
