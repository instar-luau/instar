#include "configuration.hpp"

#include "Luau/LuauConfig.h"
#include "flags.hpp"
#include "instar-bridge/src/bridge.rs.h"

#include <optional>
#include <stdexcept>
#include <string>

namespace instar::luau {
    Language configure(rust::Slice<const Language> previous, rust::Str name, rust::Str source, bool executable) {
        const Flags flags;
        Luau::Config native;

        if (!previous.empty()) {
            const auto &language = previous[0];

            native.mode = language.mode == Mode::Strict    ? Luau::Mode::Strict
                          : language.mode == Mode::NoCheck ? Luau::Mode::NoCheck
                                                           : Luau::Mode::Nonstrict;

            for (const auto &global : language.globals) {
                native.globals.emplace_back(global);
            }

            for (const auto &alias : language.aliases) {
                native.setAlias(std::string(alias.name), std::string(alias.value), std::string(alias.configuration));
            }

            native.typeErrors = language.type_errors;
        }

        Luau::ConfigOptions options;
        options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::string(name), true};
        std::optional<std::string> error;

        if (executable) {
            error = Luau::extractLuauConfig(std::string(source), native, options.aliasOptions, {});
        } else {
            error = Luau::parseConfig(std::string(source), native, options);
        }

        if (error) {
            throw std::invalid_argument(*error);
        }

        Language result;

        result.mode = native.mode == Luau::Mode::Strict    ? Mode::Strict
                      : native.mode == Luau::Mode::NoCheck ? Mode::NoCheck
                                                           : Mode::NonStrict;

        for (const auto &global : native.globals) {
            result.globals.push_back(global);
        }

        for (const auto &[name, alias] : native.aliases) {
            result.aliases.push_back({name, alias.value, std::string(alias.configLocation)});
        }

        result.type_errors = native.typeErrors;

        return result;
    }
}
