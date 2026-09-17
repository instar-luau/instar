#pragma once

#include "Luau/Frontend.h"

#include <cstdint>
#include <string>
#include <string_view>

namespace instar {

    std::string editor_query(Luau::Frontend &frontend, std::string_view module_name, uint32_t line, uint32_t column,
        std::string_view operation);

}
