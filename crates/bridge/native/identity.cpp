#include "identity.hpp"

#include <string_view>

namespace instar::luau {
    namespace {
        std::string component(std::string_view value) {
            constexpr char hexadecimal[] = "0123456789ABCDEF";
            std::string result;

            for (unsigned char character : value) {
                if ((character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z') ||
                    (character >= '0' && character <= '9') || character == '-' || character == '.' ||
                    character == '_' || character == '~') {
                    result.push_back(static_cast<char>(character));
                } else {
                    result.push_back('%');
                    result.push_back(hexadecimal[character >> 4U]);
                    result.push_back(hexadecimal[character & 15U]);
                }
            }

            return result;
        }
    }

    std::string encode(const Identity &identity) {
        if (const auto *filesystem = std::get_if<FilesystemIdentity>(&identity)) {
            return "instar://filesystem/" + component(filesystem->source);
        }

        const auto &instance = std::get<InstanceIdentity>(identity);
        auto result = "instar://instance/" + component(instance.hierarchy) + "/" + std::to_string(instance.script);

        if (instance.source) {
            result += "/source/" + component(*instance.source);
        }

        return result;
    }
}
