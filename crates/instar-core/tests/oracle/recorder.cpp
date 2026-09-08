#include "recorder.h"

#include "Luau/AstJsonEncoder.h"
#include "Luau/Common.h"
#include "Luau/Parser.h"

#include <atomic>
#include <cstdlib>
#include <fstream>
#include <string>
#include <string_view>
#include <vector>

namespace {

std::string hex(std::string_view value) {
  static constexpr char digits[] = "0123456789abcdef";
  std::string result;
  result.reserve(value.size() * 2);
  for (unsigned char byte : value) {
    result += digits[byte >> 4];
    result += digits[byte & 15];
  }
  return result;
}

std::string jsonString(std::string_view value) {
  static constexpr char digits[] = "0123456789abcdef";
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
      result += digits[byte >> 4];
      result += digits[byte & 15];
    } else
      result += char(byte);
  }
  result += '\"';
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

bool flag(std::string_view name) {
  for (Luau::FValue<bool> *value = Luau::FValue<bool>::list; value;
       value = value->next)
    if (value->name == name)
      return value->value;
  return false;
}

int integer(std::string_view name) {
  for (Luau::FValue<int> *value = Luau::FValue<int>::list; value;
       value = value->next)
    if (value->name == name)
      return value->value;
  return 0;
}

std::string recordPath() {
#ifdef _MSC_VER
  char *value = nullptr;
  size_t size = 0;
  if (_dupenv_s(&value, &size, "INSTAR_ORACLE_RECORD") != 0 || !value)
    return {};
  std::string result(value);
  std::free(value);
  return result;
#else
  const char *value = std::getenv("INSTAR_ORACLE_RECORD");
  return value ? value : "";
#endif
}

// AstJsonEncoder omits attribute arguments and complete conditional-local data.
struct AstSupplement : Luau::AstVisitor {
  std::string json = "[";
  bool first = true;

  void append(const std::string &value) {
    if (!first)
      json += ',';
    first = false;
    json += value;
  }

  bool visit(Luau::AstStatIf *node) override {
    if (auto *local = node->conditionLocal) {
      const auto &at = local->location;
      std::string span = std::to_string(at.begin.line) + "," +
                         std::to_string(at.begin.column) + " - " +
                         std::to_string(at.end.line) + "," +
                         std::to_string(at.end.column);
      append("{\"name\":" + jsonString(local->name.value) +
             ",\"type\":\"AstLocal\",\"location\":" + jsonString(span) + "}");
    }
    return true;
  }

  void attributes(const Luau::AstArray<Luau::AstAttr *> &values) {
    for (auto *attribute : values)
      for (auto *argument : attribute->args) {
        append(Luau::toJson(argument));
        argument->visit(this);
      }
  }

  bool visit(Luau::AstExprFunction *node) override {
    attributes(node->attributes);
    return true;
  }
  bool visit(Luau::AstStatDeclareFunction *node) override {
    attributes(node->attributes);
    return true;
  }
  bool visit(Luau::AstTypeFunction *node) override {
    attributes(node->attributes);
    return true;
  }
};

std::atomic<std::uint64_t> invocationCounter{0};
thread_local std::vector<std::uint64_t> activeParses;

} // namespace

std::uint64_t instarParseBegin() {
  const auto id = ++invocationCounter;
  activeParses.push_back(id);
  const auto path = recordPath();
  if (!path.empty()) {
    std::ofstream output(path, std::ios::app | std::ios::binary);
    output << "{\"mode\":\"parse_begin\",\"id\":" << id << "}\n";
  }
  return id;
}

void instarRecordParsed(std::uint64_t invocation, const char *source,
                        size_t size, const Luau::ParseOptions &options,
                        Luau::AstNode *root,
                        const std::vector<Luau::ParseError> &errors,
                        const std::vector<Luau::Comment> &comments,
                        const char *entry) {
  if (activeParses.empty() || activeParses.back() != invocation)
    std::abort();
  activeParses.pop_back();
  const std::string path = recordPath();
  if (path.empty())
    return;

  std::ofstream output(path, std::ios::app | std::ios::binary);
  output << "{\"mode\":\"parse\",\"id\":" << invocation
         << ",\"entry\":" << jsonString(entry) << ",\"source_hex\":\""
         << hex(std::string_view(source, size)) << "\",\"features\":{";
  output << "\"declarations\":"
         << (options.allowDeclarationSyntax ? "true" : "false");
  for (std::string_view name : {
           "DebugLuauUserDefinedClasses",
           "DebugLuauIfLocalSyntax",
           "DebugLuauNoInline",
           "LuauExportValueSyntax",
           "LuauIntegerType2",
       })
    output << ',' << jsonString(name) << ':' << (flag(name) ? "true" : "false");

  output << "},\"limits\":{";
  bool first = true;
  for (std::string_view name :
       {"LuauRecursionLimit", "LuauTypeLengthLimit", "LuauParseErrorLimit"}) {
    if (!first)
      output << ',';
    first = false;
    output << jsonString(name) << ':' << integer(name);
  }
  output << "},\"errors\":[";
  for (size_t index = 0; index < errors.size(); ++index) {
    if (index)
      output << ',';
    const Luau::ParseError &error = errors[index];
    output << "{\"location\":" << location(error.getLocation())
           << ",\"message\":" << jsonString(error.getMessage()) << '}';
  }
  output << "],\"ast\":";
  if (errors.empty()) {
    output << jsonString(Luau::toJson(root, comments));
    AstSupplement supplement;
    root->visit(&supplement);
    output << ",\"ast_supplement\":" << jsonString(supplement.json + "]");
  } else
    output << "null";
  output << "}\n";
}

void instarRecordLex(const char *source, size_t size, Luau::Position start) {
  const std::string path = recordPath();
  if (path.empty())
    return;
  std::ofstream output(path, std::ios::app | std::ios::binary);
  output << "{\"mode\":"
         << jsonString(activeParses.empty() ? "lex" : "parser_lex")
         << ",\"parent\":" << (activeParses.empty() ? 0 : activeParses.back())
         << ",\"start\":" << position(start) << ",\"source_hex\":\""
         << hex(std::string_view(source, size))
         << "\",\"integer\":" << (flag("LuauIntegerType2") ? "true" : "false")
         << "}\n";
}
