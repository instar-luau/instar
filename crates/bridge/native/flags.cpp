#include "flags.hpp"

#include "Luau/Common.h"
#include "Luau/Flags.h"
#include "instar-bridge/src/bridge.rs.h"

#include <mutex>
#include <stdexcept>
#include <utility>
#include <vector>

namespace instar::luau {
    namespace {
        using Value = std::variant<bool, int>;
        using Values = std::map<std::string, Value>;

        struct State {
            std::mutex mutex;
            std::size_t users = 0;
            Values active;
            std::vector<std::pair<Luau::FValue<bool> *, bool>> booleans;
            std::vector<std::pair<Luau::FValue<int> *, int>> integers;

            State() {
                setLuauFlagsDefault();

                for (auto *flag = Luau::FValue<bool>::list; flag; flag = flag->next) {
                    booleans.emplace_back(flag, flag->value);
                }

                for (auto *flag = Luau::FValue<int>::list; flag; flag = flag->next) {
                    integers.emplace_back(flag, flag->value);
                }
            }
        };

        State &state() {
            static State instance;

            return instance;
        }

        Value value(const Flag &flag) {
            switch (flag.kind) {
            case FlagKind::Boolean:
                return flag.is_enabled;

            case FlagKind::Integer:
                return flag.integer;
            }

            throw std::invalid_argument("Invalid native flag kind.");
        }

        template <typename Type>
        const std::pair<Luau::FValue<Type> *, Type> &
        registered(const std::string &name, const std::vector<std::pair<Luau::FValue<Type> *, Type>> &defaults) {
            for (const auto &entry : defaults) {
                if (name == entry.first->name) {
                    return entry;
                }
            }

            throw std::invalid_argument("Luau flag `" + name + "` is unregistered or has an incorrect value type.");
        }

        Values normalize(rust::Slice<const Flag> flags, const State &shared) {
            Values values;

            for (const Flag &flag : flags) {
                const std::string name(flag.name);

                if (!values.emplace(name, value(flag)).second) {
                    throw std::invalid_argument("Duplicate Luau flag `" + name + "`.");
                }
            }

            for (auto setting = values.begin(); setting != values.end();) {
                const Value initial = std::holds_alternative<bool>(setting->second)
                                          ? Value(registered(setting->first, shared.booleans).second)
                                          : Value(registered(setting->first, shared.integers).second);

                if (setting->second == initial) {
                    setting = values.erase(setting);
                } else {
                    ++setting;
                }
            }

            return values;
        }

        void validate(const Values &values, const Values &scoped) {
            if (scoped.size() != values.size()) {
                throw std::invalid_argument("Module flags differ from the frontend flag settings.");
            }

            for (const auto &[name, setting] : scoped) {
                const auto found = values.find(name);

                if (found == values.end() || found->second != setting) {
                    throw std::invalid_argument("Module flag `" + name + "` differs from the frontend flag settings.");
                }
            }
        }
    }

    Flags::Flags() : mutex(state().mutex), users(state().users) {
        std::lock_guard<std::mutex> lock(mutex);
        values = state().active;
        ++users;
    }

    Flags::Flags(rust::Slice<const Flag> flags) : mutex(state().mutex), users(state().users) {
        State &shared = state();
        std::lock_guard<std::mutex> lock(shared.mutex);
        values = normalize(flags, shared);

        if (shared.users != 0 && values != shared.active) {
            throw FlagConflict("Different Luau flag settings are already in use by another frontend.");
        }

        if (shared.users == 0) {
            for (const auto &[flag, initial] : shared.booleans) {
                flag->value = initial;
            }

            for (const auto &[flag, initial] : shared.integers) {
                flag->value = initial;
            }

            for (const auto &[name, setting] : values) {
                if (const auto *boolean = std::get_if<bool>(&setting)) {
                    registered(name, shared.booleans).first->value = *boolean;
                } else {
                    registered(name, shared.integers).first->value = std::get<int>(setting);
                }
            }

            shared.active = values;
        }

        ++shared.users;
    }

    Flags::~Flags() {
        std::lock_guard<std::mutex> lock(mutex);
        --users;
    }

    void Flags::require(rust::Slice<const Flag> flags) const {
        validate(values, normalize(flags, state()));
    }
}
