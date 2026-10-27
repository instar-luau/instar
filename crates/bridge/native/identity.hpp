#ifndef INSTAR_LUAU_NATIVE_IDENTITY
#define INSTAR_LUAU_NATIVE_IDENTITY

#include <cstddef>
#include <optional>
#include <string>
#include <variant>

namespace instar::luau {
    struct FilesystemIdentity {
        std::string source;
    };

    struct InstanceIdentity {
        std::string hierarchy;
        std::size_t script;
        std::optional<std::string> source;
    };

    using Identity = std::variant<FilesystemIdentity, InstanceIdentity>;

    std::string encode(const Identity &identity);
}

#endif
