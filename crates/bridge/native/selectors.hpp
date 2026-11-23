#ifndef INSTAR_LUAU_NATIVE_SELECTORS
#define INSTAR_LUAU_NATIVE_SELECTORS

#include <optional>
#include <string_view>
#include <vector>

namespace instar::luau {
    using SelectorClasses = std::vector<std::vector<std::string_view>>;

    std::optional<SelectorClasses> selector_classes(std::string_view selector);
}

#endif
