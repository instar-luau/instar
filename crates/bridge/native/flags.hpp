#ifndef INSTAR_LUAU_NATIVE_FLAGS
#define INSTAR_LUAU_NATIVE_FLAGS

#include "rust/cxx.h"

#include <cstddef>
#include <map>
#include <mutex>
#include <stdexcept>
#include <string>
#include <variant>

namespace instar::luau {
    struct Flag;

    struct FlagConflict : std::invalid_argument {
        using std::invalid_argument::invalid_argument;
    };

    class Flags {
    public:
        Flags();
        explicit Flags(rust::Slice<const Flag> flags);
        ~Flags();
        Flags(const Flags &) = delete;
        Flags &operator=(const Flags &) = delete;

        void require(rust::Slice<const Flag> flags) const;

    private:
        std::mutex &mutex;
        std::size_t &users;
        std::map<std::string, std::variant<bool, int>> values;
    };
}

#endif
