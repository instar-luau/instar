#include "Luau/AstJsonEncoder.h"
#include "Luau/Common.h"
#include "Luau/FileUtils.h"
#include "Luau/Lexer.h"
#include "Luau/ParseOptions.h"
#include "Luau/Parser.h"
#include "Luau/RequireNavigator.h"
#include "Luau/VfsNavigator.h"

#include <cstring>
#include <iostream>
#include <iterator>
#include <string>
#include <string_view>

namespace {

std::string jsonString(std::string_view value) {
  static constexpr char hex[] = "0123456789abcdef";
  std::string result = "\"";
  for (unsigned char byte : value) {
    if (byte == '\"')
      result += "\\\"";
    else if (byte == '\\')
      result += "\\\\";
    else if (byte == '\b')
      result += "\\b";
    else if (byte == '\f')
      result += "\\f";
    else if (byte == '\n')
      result += "\\n";
    else if (byte == '\r')
      result += "\\r";
    else if (byte == '\t')
      result += "\\t";
    else if (byte < 0x20 || byte >= 0x80) {
      result += "\\u00";
      result += hex[byte >> 4];
      result += hex[byte & 15];
    } else
      result += char(byte);
  }
  result += '\"';
  return result;
}

std::string hexString(std::string_view value) {
  static constexpr char hex[] = "0123456789abcdef";
  std::string result;
  result.reserve(value.size() * 2);
  for (unsigned char byte : value) {
    result += hex[byte >> 4];
    result += hex[byte & 15];
  }
  return result;
}

std::string position(const Luau::Position &value) {
  return "[" + std::to_string(value.line) + "," + std::to_string(value.column) +
         "]";
}

std::string location(const Luau::Location &value) {
  return "{\"begin\":" + position(value.begin) +
         ",\"end\":" + position(value.end) + "}";
}

void enableProfile(std::string_view profile, Luau::ParseOptions &options) {
  if (profile != "all")
    return;

  options.allowDeclarationSyntax = true;
  for (Luau::FValue<bool> *flag = Luau::FValue<bool>::list; flag;
       flag = flag->next) {
    if (strstr(flag->name, "Luau") != nullptr)
      flag->value = true;
  }
}

int parse(std::string_view profile, const std::string &source) {
  Luau::Allocator allocator;
  Luau::AstNameTable names(allocator);
  Luau::ParseOptions options;
  options.captureComments = true;
  enableProfile(profile, options);
  Luau::ParseResult result = Luau::Parser::parse(source.data(), source.size(),
                                                 names, allocator, options);

  std::cout << "{\"errors\":[";
  for (size_t index = 0; index < result.errors.size(); ++index) {
    if (index)
      std::cout << ',';
    const Luau::ParseError &error = result.errors[index];
    std::cout << "{\"location\":" << location(error.getLocation())
              << ",\"message\":" << jsonString(error.getMessage()) << '}';
  }
  std::cout << "],\"ast\":"
            << jsonString(Luau::toJson(result.root, result.commentLocations))
            << '}';
  return 0;
}

int lex(std::string_view profile, const std::string &source) {
  Luau::ParseOptions options;
  enableProfile(profile, options);
  Luau::Allocator allocator;
  Luau::AstNameTable names(allocator);
  Luau::Lexer lexer(source.data(), source.size(), names);
  lexer.setSkipComments(false);

  std::cout << "{\"tokens\":[";
  bool first = true;
  for (const Luau::Lexeme *token = &lexer.next();
       token->type != Luau::Lexeme::Eof; token = &lexer.next()) {
    if (!first)
      std::cout << ',';
    first = false;
    std::cout << "{\"type\":" << int(token->type)
              << ",\"location\":" << location(token->location)
              << ",\"display\":" << jsonString(token->toString());
    if (token->type == Luau::Lexeme::RawString ||
        token->type == Luau::Lexeme::QuotedString ||
        token->type == Luau::Lexeme::InterpStringBegin ||
        token->type == Luau::Lexeme::InterpStringMid ||
        token->type == Luau::Lexeme::InterpStringEnd ||
        token->type == Luau::Lexeme::InterpStringSimple ||
        token->type == Luau::Lexeme::BrokenInterpDoubleBrace ||
        token->type == Luau::Lexeme::Number ||
        token->type == Luau::Lexeme::Comment ||
        token->type == Luau::Lexeme::BlockComment) {
      std::string value(token->data, token->getLength());
      std::cout << ",\"value_hex\":\"" << hexString(value) << '\"';
      bool decoded = false;
      if (token->type == Luau::Lexeme::RawString) {
        Luau::Lexer::fixupMultilineString(value);
        decoded = true;
      } else if (token->type == Luau::Lexeme::QuotedString ||
                 token->type == Luau::Lexeme::InterpStringBegin ||
                 token->type == Luau::Lexeme::InterpStringMid ||
                 token->type == Luau::Lexeme::InterpStringEnd ||
                 token->type == Luau::Lexeme::InterpStringSimple)
        decoded = Luau::Lexer::fixupQuotedString(value);
      if (decoded)
        std::cout << ",\"decoded_hex\":\"" << hexString(value) << '\"';
    }
    std::cout << '}';
  }
  std::cout << "]}";
  return 0;
}

Luau::Require::NavigationContext::NavigateResult
convert(NavigationStatus status) {
  using Result = Luau::Require::NavigationContext::NavigateResult;
  switch (status) {
  case NavigationStatus::Success:
    return Result::Success;
  case NavigationStatus::Ambiguous:
    return Result::Ambiguous;
  case NavigationStatus::NotFound:
    return Result::NotFound;
  }
  return Result::NotFound;
}

class FileContext final : public Luau::Require::NavigationContext {
public:
  explicit FileContext(std::string requirer) : requirer(std::move(requirer)) {}

  NavigateResult resetToRequirer() override {
    return convert(vfs.resetToPath(requirer));
  }

  NavigateResult jumpToAlias(const std::string &path) override {
    return isAbsolutePath(path) ? convert(vfs.resetToPath(path))
                                : NavigateResult::NotFound;
  }

  NavigateResult toParent() override { return convert(vfs.toParent()); }

  NavigateResult toChild(const std::string &component) override {
    return convert(vfs.toChild(component));
  }

  bool isModulePresent() const { return isFile(vfs.getAbsoluteFilePath()); }

  std::string path() const { return vfs.getAbsoluteFilePath(); }

private:
  std::string requirer;
  VfsNavigator vfs;
};

class ErrorHandler final : public Luau::Require::ErrorHandler {
public:
  void reportError(std::string value) override { message = std::move(value); }

  std::string message;
};

int resolve(const std::string &requirer, const std::string &request) {
  FileContext context(requirer);
  ErrorHandler errors;
  Luau::Require::Navigator navigator(context, errors);
  if (navigator.navigate(request) !=
      Luau::Require::Navigator::Status::Success) {
    std::cout << "{\"status\":\"error\",\"message\":"
              << jsonString(errors.message) << '}';
  } else if (!context.isModulePresent()) {
    std::cout << "{\"status\":\"missing\",\"path\":"
              << jsonString(context.path()) << '}';
  } else {
    std::cout << "{\"status\":\"resolved\",\"path\":"
              << jsonString(context.path()) << '}';
  }
  return 0;
}

} // namespace

int main(int argc, char **argv) {
  if (argc >= 2 &&
      (strcmp(argv[1], "parse") == 0 || strcmp(argv[1], "lex") == 0)) {
    if (argc != 3)
      return 2;
    std::string source{std::istreambuf_iterator<char>(std::cin),
                       std::istreambuf_iterator<char>()};
    return strcmp(argv[1], "parse") == 0 ? parse(argv[2], source)
                                         : lex(argv[2], source);
  }
  if (argc == 4 && strcmp(argv[1], "resolve") == 0)
    return resolve(argv[2], argv[3]);
  return 2;
}
