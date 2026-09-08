// Included inside the generated AstJsonEncoder, where PROP and writeNode live.
// Upstream's default visitor flattens these nodes instead of writing JSON
// objects.
bool visit(AstNode *node) override {
  writeNode(node, "UnhandledAstNode",
            [&] { write("classIndex", node->classIndex); });
  return false;
}

bool visit(AstType *node) override {
  return visit(static_cast<AstNode *>(node));
}

bool visit(AstGenericType *node) override {
  write(node);
  return false;
}

bool visit(AstGenericTypePack *node) override {
  write(node);
  return false;
}

void writeNumber(AstExprConstantNumber *node) {
  static_assert(sizeof(node->value) == sizeof(std::uint64_t));
  std::uint64_t bits;
  std::memcpy(&bits, &node->value, sizeof(bits));
  writeNode(node, "AstExprConstantNumber", [&] {
    write("value", node->value);
    write("bits", static_cast<unsigned long long>(bits));
  });
}

bool visit(AstExprInstantiate *node) override {
  writeNode(node, "AstExprInstantiate", [&] {
    PROP(expr);
    PROP(typeArguments);
  });
  return false;
}

bool visit(AstStatTypeFunction *node) override {
  writeNode(node, "AstStatTypeFunction", [&] {
    PROP(name);
    PROP(nameLocation);
    PROP(body);
    PROP(exported);
    PROP(hasErrors);
  });
  return false;
}

bool visit(AstAttr *node) override {
  writeNode(node, "AstAttr", [&] {
    write("attributeType", int(node->type));
    PROP(name);
    PROP(args);
  });
  return false;
}

void write(const AstClassMember &member) {
  writeRaw("{");
  bool comma = pushComma();
  if (const auto *property = member.get_if<AstClassProperty>()) {
    writeType("AstClassProperty");
    write("qualifierLocation", property->qualifierLocation);
    write("name", property->name);
    write("nameLocation", property->nameLocation);
    if (property->typeColonLocation)
      write("typeColonLocation", *property->typeColonLocation);
    if (property->ty)
      write("luauType", property->ty);
    else
      write("luauType", nullptr);
  } else if (const auto *method = member.get_if<AstClassMethod>()) {
    writeType("AstClassMethod");
    if (method->qualifierLocation)
      write("qualifierLocation", *method->qualifierLocation);
    write("keywordLocation", method->keywordLocation);
    write("name", method->functionName);
    write("nameLocation", method->nameLocation);
    write("function", method->function);
  } else {
    LUAU_ASSERT(!"unhandled class member");
    write("unhandled", true);
  }
  popComma(comma);
  writeRaw("}");
}

bool visit(AstStatClass *node) override {
  writeNode(node, "AstStatClass", [&] {
    PROP(name);
    if (node->super)
      PROP(super);
    else
      write("super", nullptr);
    PROP(members);
    PROP(exported);
    PROP(open);
  });
  return false;
}
