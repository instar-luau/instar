#include "Luau/Compiler.h"
#include "Luau/JsonEmitter.h"
#include "lua.h"
#include "lualib.h"

#include <cmath>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>
#include <unordered_set>

void instar_initialize();

extern "C" {
struct Bytes {
  const char *data;
  size_t size;
};

using Reply = void (*)(void *, Bytes, bool);
}

static void append(std::string &output, std::string_view value, size_t limit) {
  if (value.size() > limit - output.size())
    throw std::runtime_error("graft response exceeds the payload limit");

  output.append(value);
}

static void serialize(lua_State *state, int index, std::string &output,
                      size_t limit, std::unordered_set<const void *> &active,
                      unsigned depth) {
  if (depth >= 128 || !lua_checkstack(state, 3))
    throw std::runtime_error("graft response exceeds the JSON nesting limit");

  index = lua_absindex(state, index);

  switch (lua_type(state, index)) {
  case LUA_TNIL:
    append(output, "null", limit);

    return;

  case LUA_TBOOLEAN:
    append(output, lua_toboolean(state, index) ? "true" : "false", limit);

    return;

  case LUA_TNUMBER: {
    double value = lua_tonumber(state, index);

    if (!std::isfinite(value) || std::trunc(value) != value || value < 0 ||
        value >=
            static_cast<double>(std::numeric_limits<unsigned long long>::max()))
      throw std::runtime_error(
          "graft response numbers must be nonnegative integers");

    append(output, std::to_string(static_cast<unsigned long long>(value)),
           limit);

    return;
  }

  case LUA_TSTRING: {
    size_t size;
    const char *value = lua_tolstring(state, index, &size);

    if (size > limit - output.size())
      throw std::runtime_error("graft response exceeds the payload limit");

    Luau::Json::JsonEmitter emitter;
    Luau::Json::write(emitter, std::string_view(value, size));
    append(output, emitter.str(), limit);

    return;
  }

  case LUA_TTABLE:
    break;

  default:
    throw std::runtime_error("graft response contains an unsupported value");
  }

  const void *identity = lua_topointer(state, index);

  if (!active.insert(identity).second)
    throw std::runtime_error("graft response contains a cycle");

  if (lua_getmetatable(state, index))
    throw std::runtime_error("graft response tables must have no metatable");

  int length = lua_objlen(state, index);
  size_t count = 0;
  bool array = true;
  bool object = true;
  lua_pushnil(state);

  while (lua_next(state, index)) {
    ++count;

    if (lua_type(state, -2) == LUA_TSTRING)
      array = false;
    else if (lua_type(state, -2) == LUA_TNUMBER) {
      object = false;
      double key = lua_tonumber(state, -2);

      if (key < 1 || key > length || std::trunc(key) != key)
        array = false;
    } else
      array = object = false;

    lua_pop(state, 1);
  }

  array = array && count == static_cast<size_t>(length);

  if (!array && !object)
    throw std::runtime_error(
        "graft response tables must be records or dense arrays");

  append(output, array ? "[" : "{", limit);

  if (array) {
    for (int position = 0; position < length; ++position) {
      if (position)
        append(output, ",", limit);

      lua_rawgeti(state, index, position + 1);
      serialize(state, -1, output, limit, active, depth + 1);
      lua_pop(state, 1);
    }
  } else {
    bool first = true;
    lua_pushnil(state);

    while (lua_next(state, index)) {
      if (!first)
        append(output, ",", limit);

      first = false;
      serialize(state, -2, output, limit, active, depth + 1);
      append(output, ":", limit);
      serialize(state, -1, output, limit, active, depth + 1);
      lua_pop(state, 1);
    }
  }

  append(output, array ? "]" : "}", limit);
  active.erase(identity);
}

static void run(lua_State *state, Bytes source, const char *name) {
  std::string bytecode = Luau::compile(std::string(source.data, source.size));

  if (luau_load(state, name, bytecode.data(), bytecode.size(), 0) != 0 ||
      lua_pcall(state, 0, 1, 0) != 0) {
    const char *error = lua_tostring(state, -1);

    throw std::runtime_error(error ? error
                                   : "Luau graft raised a non-string error");
  }
}

extern "C" void instar_graft(Bytes source, Bytes request, bool format,
                             bool lint, size_t limit, void *context,
                             Reply reply) {
  try {
    instar_initialize();

    std::unique_ptr<lua_State, void (*)(lua_State *)> owner(luaL_newstate(),
                                                            lua_close);

    if (!owner)
      throw std::runtime_error("cannot allocate Luau graft state");

    lua_State *state = owner.get();
    luaL_openlibs(state);
    lua_pushnil(state);
    lua_setglobal(state, "print");
    luaL_sandbox(state);
    luaL_sandboxthread(state);
    run(state, source, "=graft");

    if (!lua_istable(state, -1))
      throw std::runtime_error("Luau graft must return a module table");

    for (const char *hook : {"format", "lint"}) {
      if ((std::string_view(hook) == "format" && !format) ||
          (std::string_view(hook) == "lint" && !lint))
        continue;

      lua_rawgetfield(state, 1, hook);

      if (!lua_isfunction(state, -1))
        throw std::runtime_error(std::string("Luau graft exports no ") + hook +
                                 " function");

      lua_pop(state, 1);
    }

    if (!request.size) {
      reply(context, {nullptr, 0}, true);

      return;
    }

    run(state, request, "=request");
    lua_rawgetfield(state, 2, "hook");
    std::string hook = lua_tostring(state, -1);
    lua_pop(state, 1);
    lua_rawgetfield(state, 1, hook.c_str());
    lua_pushvalue(state, 2);

    if (lua_pcall(state, 1, 1, 0) != 0) {
      const char *error = lua_tostring(state, -1);

      throw std::runtime_error(error ? error
                                     : "Luau graft raised a non-string error");
    }

    std::string output;
    std::unordered_set<const void *> active;
    serialize(state, -1, output, limit, active, 0);
    reply(context, {output.data(), output.size()}, true);
  } catch (const std::exception &error) {
    std::string message = error.what();
    reply(context, {message.data(), message.size()}, false);
  } catch (...) {
    const std::string message = "Luau graft raised a native exception";
    reply(context, {message.data(), message.size()}, false);
  }
}
