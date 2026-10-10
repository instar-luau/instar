#include "configuration.hpp"

#include "Luau/Config.h"
#include "Luau/LuauConfig.h"
#include "instar-bridge/src/boundary.rs.h"
#include "lua.h"

#include <chrono>
#include <stdexcept>
#include <string>
#include <unordered_set>

namespace instar {
    namespace {
        struct DeadlineExceeded final : std::runtime_error {
            DeadlineExceeded() : std::runtime_error("configuration execution timed out") {}
        };

        struct ExecutionCancelled final : std::runtime_error {
            ExecutionCancelled() : std::runtime_error("configuration execution cancelled") {}
        };

        struct ExtractionFailure final : std::runtime_error {
            using std::runtime_error::runtime_error;
        };

        struct Execution {
            std::chrono::steady_clock::time_point started;
            double timeout_seconds;
            const Cancellation &cancellation;
            lua_State *main = nullptr;
            size_t entries = 0;
            std::unordered_set<const void *> ancestors;
            bool running = false;

            void check() const {
                if (cancellation.requested()) {
                    throw ExecutionCancelled{};
                }

                const auto elapsed = std::chrono::duration<double>(std::chrono::steady_clock::now() - started);

                if (elapsed.count() >= timeout_seconds) {
                    throw DeadlineExceeded{};
                }
            }

            void inspect(lua_State *state, size_t depth) {
                check();

                if (depth > 128) {
                    throw ExtractionFailure{"configuration table exceeds extraction depth limit (128)"};
                }

                const void *identity = lua_topointer(state, -1);

                if (!ancestors.insert(identity).second) {
                    throw ExtractionFailure{"configuration table contains a cycle"};
                }

                if (!lua_checkstack(state, 3)) {
                    throw ExtractionFailure{"configuration table exceeds extraction stack limit"};
                }

                const int table = lua_gettop(state);
                lua_pushnil(state);

                while (lua_next(state, table) != 0) {
                    check();

                    if (++entries > 100000) {
                        throw ExtractionFailure{"configuration table exceeds extraction entry limit (100000)"};
                    }

                    if (lua_type(state, -1) == LUA_TTABLE) {
                        inspect(state, depth + 1);
                    }

                    lua_pop(state, 1);
                }

                ancestors.erase(identity);
            }
        };

        const char *mode_name(Luau::Mode mode) {
            switch (mode) {
            case Luau::Mode::NoCheck:
                return "nocheck";

            case Luau::Mode::Nonstrict:
                return "nonstrict";

            case Luau::Mode::Strict:
                return "strict";

            case Luau::Mode::Definition:
                return "definition";
            }

            throw std::runtime_error("unknown upstream language mode");
        }
    }

    NativeConfiguration::NativeConfiguration() : configuration(std::make_unique<Luau::Config>()) {}

    NativeConfiguration::~NativeConfiguration() = default;

    std::unique_ptr<NativeConfiguration> create() {
        return std::make_unique<NativeConfiguration>();
    }

    NativeOutcome NativeConfiguration::apply(
        rust::Str source, rust::Str path, bool executable, double timeout_seconds, const Cancellation &cancellation
    ) {
        Execution execution{std::chrono::steady_clock::now(), timeout_seconds, cancellation, nullptr, 0, {}};
        auto next = std::make_unique<Luau::Config>(*configuration);
        Luau::ConfigOptions::AliasOptions aliases{std::string(path), true};
        std::optional<std::string> error;

        try {
            execution.check();

            if (executable) {
                Luau::InterruptCallbacks callbacks;

                callbacks.initCallback = [&execution](lua_State *state) {
                    lua_setsafeenv(state, LUA_GLOBALSINDEX, false);
                    lua_setreadonly(state, LUA_GLOBALSINDEX, false);
                    lua_pushnil(state);
                    lua_setglobal(state, "print");
                    lua_setreadonly(state, LUA_GLOBALSINDEX, true);

                    execution.main = state;
                    lua_callbacks(state)->userdata = &execution;

                    lua_callbacks(state)->postresume = [](lua_State *resumed) {
                        auto &execution = *static_cast<Execution *>(lua_callbacks(resumed)->userdata);

                        if (resumed != execution.main) {
                            return;
                        }

                        execution.check();

                        if (lua_status(resumed) == LUA_OK && lua_gettop(resumed) == 1 &&
                            lua_type(resumed, -1) == LUA_TTABLE) {
                            if (!lua_checkstack(resumed, 128 * 3 + 3)) {
                                throw ExtractionFailure{"configuration table exceeds extraction stack limit"};
                            }

                            execution.inspect(resumed, 1);
                        }
                    };
                };

                callbacks.interruptCallback = [](lua_State *state, int) {
                    auto &execution = *static_cast<Execution *>(lua_callbacks(state)->userdata);

                    if (!execution.running) {
                        execution.running = true;
                        execution.cancellation.started();
                    }

                    execution.check();
                };

                error = Luau::extractLuauConfig(std::string(source), *next, aliases, std::move(callbacks));
            } else {
                Luau::ConfigOptions options;
                options.aliasOptions = std::move(aliases);
                error = Luau::parseConfig(std::string(source), *next, options);
            }

            execution.check();
        } catch (const DeadlineExceeded &failure) {
            return NativeOutcome{rust::String(failure.what()), true, false};
        } catch (const ExecutionCancelled &failure) {
            return NativeOutcome{rust::String(failure.what()), false, true};
        } catch (const ExtractionFailure &failure) {
            return NativeOutcome{rust::String(failure.what()), false, false};
        }

        if (error) {
            return NativeOutcome{rust::String(*error), false, false};
        }

        for (const auto &[name, alias] : next->aliases) {
            if (name == "self") {
                return NativeOutcome{rust::String("alias @self is reserved"), false, false};
            }

            if (!name.empty() && name.front() == '@') {
                return NativeOutcome{rust::String("Invalid alias " + alias.originalCase), false, false};
            }
        }

        if (cancellation.requested()) {
            return NativeOutcome{rust::String("configuration execution cancelled"), false, true};
        }

        configuration.swap(next);

        return NativeOutcome{rust::String(), false, false};
    }

    NativeSnapshot NativeConfiguration::snapshot() const {
        NativeSnapshot result;
        result.mode = mode_name(configuration->mode);
        result.lint_errors = configuration->lintErrors;
        result.type_errors = configuration->typeErrors;

        for (int index = Luau::LintWarning::Code_Unknown + 1; index < Luau::LintWarning::Code__Count; ++index) {
            const auto code = static_cast<Luau::LintWarning::Code>(index);

            result.lint.push_back(
                NativeLint{rust::String(Luau::LintWarning::getName(code)),
                    configuration->enabledLint.isEnabled(code),
                    configuration->fatalLint.isEnabled(code)}
            );
        }

        for (const std::string &global : configuration->globals) {
            result.globals.push_back(rust::String(global));
        }

        for (const auto &[name, alias] : configuration->aliases) {
            result.aliases.push_back(
                NativeAlias{rust::String(name),
                    rust::String(alias.value),
                    rust::String(alias.configLocation.data(), alias.configLocation.size()),
                    rust::String(alias.originalCase)}
            );
        }

        return result;
    }

    void NativeConfiguration::restore(const NativeSnapshot &snapshot) {
        auto next = std::make_unique<Luau::Config>();

        if (const auto error = Luau::parseModeString(next->mode, std::string(snapshot.mode))) {
            throw std::invalid_argument(*error);
        }

        next->lintErrors = snapshot.lint_errors;
        next->typeErrors = snapshot.type_errors;
        next->enabledLint.warningMask = 0;
        next->fatalLint.warningMask = 0;

        for (const NativeLint &lint : snapshot.lint) {
            const auto code = Luau::LintWarning::parseName(std::string(lint.name).c_str());

            if (code == Luau::LintWarning::Code_Unknown) {
                throw std::invalid_argument("unknown native lint name");
            }

            if (lint.enabled) {
                next->enabledLint.enableWarning(code);
            }

            if (lint.fatal) {
                next->fatalLint.enableWarning(code);
            }
        }

        for (const rust::String &global : snapshot.globals) {
            next->globals.emplace_back(global);
        }

        for (const NativeAlias &alias : snapshot.aliases) {
            next->setAlias(std::string(alias.original_case), std::string(alias.value), std::string(alias.location));
        }

        configuration = std::move(next);
    }

    const Luau::Config &NativeConfiguration::value() const {
        return *configuration;
    }
}
