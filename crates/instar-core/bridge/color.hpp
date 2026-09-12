#pragma once

#include "Luau/Ast.h"
#include "Luau/JsonEmitter.h"
#include <algorithm>
#include <cmath>
#include <optional>
#include <string>
#include <vector>

struct Colors : Luau::AstVisitor {
  Luau::Json::JsonEmitter &output;
  explicit Colors(Luau::Json::JsonEmitter &output) : output(output) {}

  static std::optional<double> number(Luau::AstExpr *expression) {
    if (auto *value = expression->as<Luau::AstExprConstantNumber>())
      return std::isfinite(value->value) ? std::optional<double>{value->value} : std::nullopt;

    if (auto *group = expression->as<Luau::AstExprGroup>())
      return number(group->expr);

    if (auto *unary = expression->as<Luau::AstExprUnary>(); unary && unary->op == Luau::AstExprUnary::Op::Minus)
      if (auto value = number(unary->expr))
        return -*value;

    return std::nullopt;
  }

  bool visit(Luau::AstExprCall *call) override {
    auto *member = call->func->as<Luau::AstExprIndexName>();

    if (!member || member->op != '.')
      return true;

    auto *global = member->expr->as<Luau::AstExprGlobal>();

    if (!global || global->name != "Color3")
      return true;

    std::vector<double> channels;

    if ((member->index == "new" || member->index == "fromRGB") && call->args.size == 3) {
      for (auto *argument : call->args) {
        auto value = number(argument);

        if (!value)
          return true;

        channels.push_back(std::clamp(*value / (member->index == "fromRGB" ? 255.0 : 1.0), 0.0, 1.0));
      }
    } else if (member->index == "fromHex" && call->args.size == 1) {
      auto *literal = call->args.data[0]->as<Luau::AstExprConstantString>();

      if (!literal)
        return true;

      std::string digits(literal->value.data, literal->value.size);

      if (!digits.empty() && digits.front() == '#')
        digits.erase(0, 1);

      if ((digits.size() != 3 && digits.size() != 6) || digits.find_first_not_of("0123456789abcdefABCDEF") != std::string::npos)
        return true;

      size_t width = digits.size() / 3;

      for (size_t index = 0; index < 3; ++index) {
        auto value = std::stoul(digits.substr(index * width, width), nullptr, 16);
        channels.push_back(double(value * (width == 1 ? 17 : 1)) / 255.0);
      }
    } else
      return true;

    channels.push_back(1.0);
    output.writeComma();
    auto object = output.writeObject();
    object.writePair("range", std::vector<unsigned>{call->location.begin.line, call->location.begin.column, call->location.end.line, call->location.end.column});
    object.writePair("color", channels);

    return true;
  }
};
