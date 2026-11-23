#include "selectors.hpp"

#include <cmath>
#include <cstdlib>
#include <string>

namespace instar::luau {
    namespace {
        bool letter(char character) {
            return (character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z');
        }

        bool digit(char character) {
            return character >= '0' && character <= '9';
        }

        bool identifier_character(char character) {
            return letter(character) || digit(character) || character == '_' || character == '-';
        }

        std::optional<double> number(std::string_view token) {
            if (token.empty() || (!digit(token.front()) && token.front() != '-' && token.front() != '.')) {
                return std::nullopt;
            }

            const std::string text(token);
            char *end = nullptr;
            const double value = std::strtod(text.c_str(), &end);

            if (end != text.c_str() + text.size() || !std::isfinite(value)) {
                return std::nullopt;
            }

            return value;
        }

        class Parser {
        public:
            explicit Parser(std::string_view input) : remaining(input) {}

            std::optional<SelectorClasses> parse() {
                whitespace();
                SelectorClasses classes;

                if (remaining.empty() || (list(true, 0, &classes) && remaining.empty())) {
                    return classes;
                }

                return std::nullopt;
            }

        private:
            std::string_view remaining;

            char front() const {
                return remaining.empty() ? '\0' : remaining.front();
            }

            bool take(char character) {
                if (remaining.empty() || front() != character) {
                    return false;
                }

                remaining.remove_prefix(1);

                return true;
            }

            void whitespace() {
                while (front() == ' ' || front() == '\t' || front() == '\n' || front() == '\r') {
                    remaining.remove_prefix(1);
                }
            }

            std::string_view identifier() {
                if (!letter(front()) && front() != '_') {
                    return {};
                }

                const auto start = remaining;

                while (identifier_character(front())) {
                    remaining.remove_prefix(1);
                }

                return start.substr(0, start.size() - remaining.size());
            }

            std::string_view value_token() {
                const auto start = remaining;

                while (identifier_character(front()) || front() == '.' || front() == '+') {
                    remaining.remove_prefix(1);
                }

                return start.substr(0, start.size() - remaining.size());
            }

            bool value() {
                if (take('"')) {
                    while (!remaining.empty() && front() != '"') {
                        if (front() == '\\' || static_cast<unsigned char>(front()) < ' ') {
                            return false;
                        }

                        remaining.remove_prefix(1);
                    }

                    return take('"');
                }

                const auto token = value_token();

                return (!token.empty() && token.find_first_not_of(
                                              "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
                                          ) == std::string_view::npos) ||
                       number(token).has_value();
            }

            bool comparison() {
                whitespace();
                const bool attribute = take('$');

                if (identifier().empty()) {
                    return false;
                }

                whitespace();

                if (attribute && take(']')) {
                    return true;
                }

                if (!take('=')) {
                    return false;
                }

                whitespace();

                if (!value()) {
                    return false;
                }

                whitespace();

                return take(']');
            }

            bool pseudo(unsigned depth) {
                const auto name = identifier();

                if (depth == 64 || !take('(')) {
                    return false;
                }

                whitespace();

                if (name == "in-radius") {
                    const auto radius = number(value_token());
                    whitespace();

                    if (!radius || *radius < 0 || !take(',')) {
                        return false;
                    }

                    whitespace();
                } else if (name != "has" && name != "not") {
                    return false;
                }

                return list(name != "not", depth + 1, nullptr) && take(')');
            }

            bool compound(unsigned depth, std::vector<std::string_view> *classes) {
                bool found = false;

                while (true) {
                    if (take('.') || take('#')) {
                        if (identifier().empty()) {
                            return false;
                        }
                    } else if (take('[')) {
                        if (!comparison()) {
                            return false;
                        }
                    } else if (take(':')) {
                        if (!pseudo(depth)) {
                            return false;
                        }
                    } else if (const auto name = identifier(); !name.empty()) {
                        if (classes) {
                            classes->push_back(name);
                        }
                    } else {
                        return found;
                    }

                    found = true;
                    whitespace();
                }
            }

            bool combinator() {
                if (!take('>')) {
                    return false;
                }

                take('>');
                whitespace();

                return true;
            }

            bool list(bool relative, unsigned depth, SelectorClasses *classes) {
                do {
                    whitespace();

                    if (relative) {
                        combinator();
                    }

                    auto *branch = classes ? &classes->emplace_back() : nullptr;

                    do {
                        if (branch) {
                            branch->clear();
                        }

                        if (!compound(depth, branch)) {
                            return false;
                        }
                    } while (combinator());
                } while (take(','));

                return true;
            }
        };
    }

    std::optional<SelectorClasses> selector_classes(std::string_view selector) {
        return Parser(selector).parse();
    }
}
