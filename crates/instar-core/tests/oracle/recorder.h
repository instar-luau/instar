#pragma once

#include "Luau/Lexer.h"
#include "Luau/ParseOptions.h"
#include "Luau/ParseResult.h"

class InstarRecordingLexer final : public Luau::Lexer {
public:
  InstarRecordingLexer(const char *source, size_t size,
                       Luau::AstNameTable &names);
};

Luau::ParseResult instarOracleParse(const char *source, size_t size,
                                    Luau::AstNameTable &names,
                                    Luau::Allocator &allocator,
                                    Luau::ParseOptions options);
