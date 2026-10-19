#ifndef INSTAR_LUAU_NATIVE_FLAGS
#define INSTAR_LUAU_NATIVE_FLAGS

#include "rust/cxx.h"

#include <cstddef>
#include <map>
#include <mutex>
#include <string>
#include <variant>

namespace instar::luau {
    struct Flag;

    class Flags {
    public:
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

    rust::Vec<bool>
    selectors(rust::Slice<const Flag> settings, rust::Slice<const Flag> scoped, rust::Slice<const rust::Str> names);
}

#endif
