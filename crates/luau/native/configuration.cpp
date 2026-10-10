#include "configuration.hpp"
#include "flags.hpp"
#include "lualib.h"

#include "Luau/Compiler.h"
#include "Luau/Config.h"
#include "instar-luau/src/boundary/mod.rs.h"
#include "lua.h"

#include <cmath>
#include <cstdlib>
#include <limits>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <unordered_set>
#include <utility>

namespace instar {
    namespace {
        struct Interrupted final : std::runtime_error {
            NativeCompletion completion;

            Interrupted(NativeCompletion completion, const char *message)
                : std::runtime_error(message), completion(completion) {}
        };

        struct Execution {
            const NativeConfigurationInput &input;
            const NativeOperation &operation;
            size_t allocated = 0;
            size_t entries = 0;
            bool exhausted = false;
            std::unordered_set<const void *> ancestors;
            rust::Vec<NativeConfigurationNode> nodes;

            void check() const {
                if (operation.requested()) {
                    throw Interrupted(NativeCompletion::Cancelled, "configuration execution cancelled");
                }

                if (operation.remaining_nanoseconds() == 0) {
                    throw Interrupted(NativeCompletion::Timeout, "configuration execution timed out");
                }
            }

            static void *allocate(void *context, void *pointer, size_t previous, size_t requested) {
                auto &execution = *static_cast<Execution *>(context);

                if (!pointer) {
                    previous = 0;
                }

                if (requested == 0) {
                    std::free(pointer);
                    execution.allocated -= previous;

                    return nullptr;
                }

                const size_t retained = execution.allocated - previous;

                if (requested > execution.input.memory_bytes || retained > execution.input.memory_bytes - requested) {
                    execution.exhausted = true;

                    return nullptr;
                }

                void *replacement = std::realloc(pointer, requested);

                if (!replacement) {
                    execution.exhausted = true;

                    return nullptr;
                }

                execution.allocated = retained + requested;

                return replacement;
            }

            uint32_t extract(lua_State *state, int index, uint32_t depth) {
                check();

                if (depth > input.extraction_depth) {
                    exhausted = true;
                    throw std::runtime_error("configuration table exceeds extraction depth limit");
                }

                if (nodes.size() >= std::numeric_limits<uint32_t>::max()) {
                    exhausted = true;
                    throw std::runtime_error("configuration node count exceeds transport limit");
                }

                if (index < 0) {
                    index = lua_gettop(state) + index + 1;
                }

                NativeConfigurationNode node{};
                const uint32_t result = static_cast<uint32_t>(nodes.size());
                nodes.push_back({});

                switch (lua_type(state, index)) {
                case LUA_TSTRING: {
                    size_t length = 0;
                    const char *text = lua_tolstring(state, index, &length);
                    node.kind = 0;
                    node.text = rust::String(text, length);
                    break;
                }

                case LUA_TNUMBER:
                    node.kind = 1;
                    node.number = lua_tonumber(state, index);

                    if (!std::isfinite(node.number)) {
                        throw std::runtime_error("configuration numbers must be finite");
                    }

                    break;

                case LUA_TBOOLEAN:
                    node.kind = 2;
                    node.boolean = lua_toboolean(state, index) != 0;
                    break;

                case LUA_TTABLE: {
                    node.kind = 3;
                    const void *identity = lua_topointer(state, index);

                    if (!ancestors.insert(identity).second) {
                        throw std::runtime_error("configuration table contains a cycle");
                    }

                    if (!lua_checkstack(state, 3)) {
                        exhausted = true;
                        throw std::runtime_error("configuration extraction stack limit exhausted");
                    }

                    lua_pushnil(state);

                    while (lua_next(state, index) != 0) {
                        check();

                        if (++entries > input.extraction_entries) {
                            exhausted = true;
                            throw std::runtime_error("configuration table exceeds extraction entry limit");
                        }

                        NativeConfigurationEntry entry{};

                        if (lua_type(state, -2) == LUA_TSTRING) {
                            size_t length = 0;
                            const char *text = lua_tolstring(state, -2, &length);
                            entry.key_kind = 0;
                            entry.key_text = rust::String(text, length);
                        } else if (lua_type(state, -2) == LUA_TNUMBER) {
                            entry.key_kind = 1;
                            entry.key_number = lua_tonumber(state, -2);

                            if (!std::isfinite(entry.key_number)) {
                                throw std::runtime_error("configuration numeric keys must be finite");
                            }
                        } else {
                            throw std::runtime_error("configuration keys must be strings or numbers");
                        }

                        entry.value_index = extract(state, -1, depth + 1);
                        node.entries.push_back(std::move(entry));
                        lua_pop(state, 1);
                    }

                    ancestors.erase(identity);
                    break;
                }

                default:
                    throw std::runtime_error("configuration values must be strings, numbers, booleans or tables");
                }

                nodes[result] = std::move(node);

                return result;
            }
        };

        int initialize(lua_State *state) {
            luaL_openlibs(state);

            for (const char *name : {"print", "warn", "require", "loadfile", "dofile", "loadstring"}) {
                lua_pushnil(state);
                lua_setglobal(state, name);
            }

            luaL_sandbox(state);

            return 0;
        }

        std::string error_text(lua_State *state) {
            size_t length = 0;
            const char *text = lua_tolstring(state, -1, &length);

            return text ? std::string(text, length) : "configuration execution failed";
        }

        const NativeConfigurationNode &value(const rust::Vec<NativeConfigurationNode> &nodes, uint32_t index) {
            if (index >= nodes.size()) {
                throw std::runtime_error("configuration value index is invalid");
            }

            return nodes[index];
        }

        const NativeConfigurationNode *
        field(const rust::Vec<NativeConfigurationNode> &nodes, const NativeConfigurationNode &table, const char *name) {
            if (table.kind != 3) {
                throw std::runtime_error("configuration language settings must be a table");
            }

            for (const auto &entry : table.entries) {
                if (entry.key_kind == 0 && std::string(entry.key_text) == name) {
                    return &value(nodes, entry.value_index);
                }
            }

            return nullptr;
        }

        void validate_language(
            const NativeConfigurationInput &input, const rust::Vec<NativeConfigurationNode> &nodes, uint32_t root
        ) {
            Luau::Config configuration;
            Luau::ConfigOptions options;
            options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::string(input.base), true};

            if (!input.executable) {
                if (const auto error = Luau::parseConfig(std::string(input.text), configuration, options)) {
                    throw std::runtime_error(*error);
                }
            } else {
                const auto &settings = value(nodes, root);

                if (const auto *checking = field(nodes, settings, "checking")) {
                    if (const auto *mode = field(nodes, *checking, "mode")) {
                        if (mode->kind != 0) {
                            throw std::runtime_error("checking.mode must be a string");
                        }

                        if (const auto error = Luau::parseModeString(configuration.mode, std::string(mode->text))) {
                            throw std::runtime_error(*error);
                        }
                    }

                    if (const auto *errors = field(nodes, *checking, "type_errors"); errors && errors->kind != 2) {
                        throw std::runtime_error("checking.type_errors must be a boolean");
                    }
                }

                if (const auto *language = field(nodes, settings, "language")) {
                    if (const auto *declarations = field(nodes, *language, "declaration_syntax");
                        declarations && declarations->kind != 2) {
                        throw std::runtime_error("language.declaration_syntax must be a boolean");
                    }

                    if (const auto *flags = field(nodes, *language, "flags")) {
                        if (flags->kind != 3) {
                            throw std::runtime_error("language.flags must be a table");
                        }

                        rust::Vec<NativeFlag> selected;

                        for (const auto &entry : flags->entries) {
                            if (entry.key_kind != 0) {
                                throw std::runtime_error("language flag names must be strings");
                            }

                            const auto &flag = value(nodes, entry.value_index);
                            NativeFlag selection{};
                            selection.name = entry.key_text;

                            if (flag.kind == 2) {
                                selection.is_boolean = true;
                                selection.boolean = flag.boolean;
                            } else if (flag.kind == 1 && std::floor(flag.number) == flag.number &&
                                       flag.number >= std::numeric_limits<int32_t>::min() &&
                                       flag.number <= std::numeric_limits<int32_t>::max()) {
                                selection.integer = static_cast<int32_t>(flag.number);
                            } else {
                                throw std::runtime_error(
                                    "language flags must have boolean or signed 32-bit integer values"
                                );
                            }

                            selected.push_back(std::move(selection));
                        }

                        static_cast<void>(
                            normalize_flags(rust::Slice<const NativeFlag>(selected.data(), selected.size()))
                        );
                    }
                }

                if (const auto *environment = field(nodes, settings, "environment")) {
                    if (const auto *globals = field(nodes, *environment, "globals")) {
                        if (globals->kind != 3) {
                            throw std::runtime_error("environment.globals must be an array of strings");
                        }

                        std::unordered_set<size_t> indices;

                        for (const auto &global : globals->entries) {
                            if (global.key_kind != 1 || global.key_number < 1 ||
                                global.key_number > globals->entries.size() ||
                                std::floor(global.key_number) != global.key_number ||
                                value(nodes, global.value_index).kind != 0 ||
                                !indices.insert(static_cast<size_t>(global.key_number)).second) {
                                throw std::runtime_error("environment.globals contains invalid numeric key or value");
                            }
                        }
                    }
                }

                if (const auto *resolution = field(nodes, settings, "resolution")) {
                    if (const auto *aliases = field(nodes, *resolution, "aliases")) {
                        if (aliases->kind != 3) {
                            throw std::runtime_error("resolution.aliases must be a table");
                        }

                        for (const auto &alias : aliases->entries) {
                            const auto &target = value(nodes, alias.value_index);

                            if (alias.key_kind != 0 || target.kind != 0) {
                                throw std::runtime_error("resolution.aliases must have string keys and values");
                            }

                            if (const auto error = Luau::parseAlias(
                                    configuration,
                                    std::string(alias.key_text),
                                    std::string(target.text),
                                    options.aliasOptions
                                )) {
                                throw std::runtime_error(*error);
                            }
                        }
                    }
                }

                if (const auto *linting = field(nodes, settings, "linting")) {
                    if (const auto *errors = field(nodes, *linting, "upstream_errors"); errors && errors->kind != 2) {
                        throw std::runtime_error("linting.upstream_errors must be a boolean");
                    }

                    if (const auto *warnings = field(nodes, *linting, "warnings")) {
                        if (warnings->kind != 3) {
                            throw std::runtime_error("linting.warnings must be a table");
                        }

                        for (const auto &warning : warnings->entries) {
                            const auto &enabled = value(nodes, warning.value_index);

                            if (warning.key_kind != 0 || enabled.kind != 2) {
                                throw std::runtime_error("linting.warnings must have string keys and boolean values");
                            }

                            if (const auto error = Luau::parseLintRuleString(
                                    configuration.enabledLint,
                                    configuration.fatalLint,
                                    std::string(warning.key_text),
                                    enabled.boolean ? "true" : "false"
                                )) {
                                throw std::runtime_error(*error);
                            }
                        }
                    }
                }
            }

            for (const auto &[name, alias] : configuration.aliases) {
                if (name == "self" || name == "@self") {
                    throw std::runtime_error("alias @self is reserved");
                }

                if (!name.empty() && name.front() == '@') {
                    throw std::runtime_error("invalid module alias name " + name);
                }
            }
        }
    }

    NativeConfigurationResult
    evaluate_configuration(const NativeConfigurationInput &input, const NativeOperation &operation) {
        if (input.memory_bytes < 1024 * 1024 || input.memory_bytes > std::numeric_limits<size_t>::max() ||
            input.extraction_depth == 0 || input.extraction_depth > 128 || input.extraction_entries == 0 ||
            input.extraction_entries > 100000) {
            throw std::invalid_argument("invalid configuration resource limits");
        }

        NativeConfigurationResult result{false, 0, {}, {}, NativeCompletion::Complete};
        Execution execution{input, operation};

        try {
            execution.check();

            if (!input.executable) {
                result.nodes = input.nodes;
                result.root = input.root;
                validate_language(input, result.nodes, result.root);
            } else {
                const std::string bytecode = Luau::compile(std::string(input.text));
                execution.check();

                std::unique_ptr<lua_State, void (*)(lua_State *)> state{lua_newstate(Execution::allocate, &execution),
                    lua_close};

                if (!state) {
                    result.completion = NativeCompletion::Resource;
                    result.message = "configuration VM memory limit exhausted";

                    return result;
                }

                lua_callbacks(state.get())->userdata = &execution;

                lua_callbacks(state.get())->interrupt = [](lua_State *state, int) {
                    static_cast<Execution *>(lua_callbacks(state)->userdata)->check();
                };

                const auto initialized = lua_cpcall(state.get(), initialize, nullptr);
                execution.check();

                if (initialized != LUA_OK) {
                    execution.exhausted = execution.exhausted || initialized == LUA_ERRMEM;
                    throw std::runtime_error(error_text(state.get()));
                }

                const auto loaded = luau_load(state.get(), "=configuration", bytecode.data(), bytecode.size(), 0);
                execution.check();

                if (loaded != LUA_OK) {
                    execution.exhausted = execution.exhausted || loaded == LUA_ERRMEM;
                    throw std::runtime_error(error_text(state.get()));
                }

                operation.progress("configuration", "", 0, 1);
                const int status = lua_resume(state.get(), nullptr, 0);
                execution.check();

                if (status == LUA_YIELD || status == LUA_BREAK) {
                    throw std::runtime_error("configuration execution cannot yield");
                }

                if (status != LUA_OK) {
                    execution.exhausted = execution.exhausted || status == LUA_ERRMEM;
                    throw std::runtime_error(error_text(state.get()));
                }

                if (lua_gettop(state.get()) != 1 || lua_type(state.get(), -1) != LUA_TTABLE) {
                    throw std::runtime_error("configuration must return exactly one table");
                }

                result.root = execution.extract(state.get(), -1, 1);
                result.nodes = std::move(execution.nodes);
                validate_language(input, result.nodes, result.root);
            }

            execution.check();
            operation.progress("configuration", "", 1, 1);
            execution.check();
            result.has_values = true;
        } catch (const Interrupted &error) {
            result.message = error.what();
            result.completion = error.completion;
        } catch (const std::bad_alloc &) {
            result.message = "configuration allocation exhausted";
            result.completion = NativeCompletion::Resource;
        } catch (const std::runtime_error &error) {
            result.message = error.what();
            result.completion = execution.exhausted ? NativeCompletion::Resource : NativeCompletion::Analysis;
        } catch (const std::invalid_argument &error) {
            result.message = error.what();
            result.completion = NativeCompletion::Analysis;
        }

        if (!result.has_values) {
            result.nodes.clear();
        }

        return result;
    }
}
