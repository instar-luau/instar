#include "types.hpp"

#include "Luau/Lexer.h"
#include "Luau/StringUtils.h"
#include "Luau/StructuralTypeEquality.h"
#include "Luau/VisitType.h"

#include <algorithm>
#include <exception>
#include <optional>
#include <stdexcept>
#include <string_view>
#include <utility>

namespace instar::luau {
    namespace {
        constexpr std::size_t maximum_bytes = 32 * 1024 - 32;
        constexpr std::size_t maximum_lines = 250;
        constexpr std::size_t maximum_label_characters = 38;

        struct Limit final : std::exception {};

        class Inspection final : public Luau::TypeVisitor {
        public:
            Inspection(PresentationBudget &budget, bool is_exhaustive)
                : Luau::TypeVisitor("Presentation", false), budget(budget), is_exhaustive(is_exhaustive) {}

            bool visit(Luau::TypeId value) override {
                count();
                check(value);

                if (const auto *bound = Luau::get<Luau::BoundType>(value)) {
                    check(bound->boundTo);
                } else if (const auto *function = Luau::get<Luau::FunctionType>(value)) {
                    is_structural &= function->generics.empty() && function->genericPacks.empty() &&
                                     !function->isCheckedFunction && !function->hasSelf;

                    for (const auto &name : function->argNames) {
                        budget.check();
                        payload(name ? name->name.size() : 0);
                    }

                    for (const auto generic : function->generics) {
                        check(generic);
                        traverse(generic);
                    }

                    for (const auto generic : function->genericPacks) {
                        traverse(generic);
                    }
                } else if (const auto *table = Luau::get<Luau::TableType>(value)) {
                    is_structural &= !table->name && !table->syntheticName;
                    payload(table->name ? table->name->size() : 0);
                    payload(table->syntheticName ? table->syntheticName->size() : 0);

                    if (table->boundTo) {
                        check(*table->boundTo);

                        return true;
                    }

                    for (const auto argument : table->instantiatedTypeParams) {
                        check(argument);
                        traverse(argument);
                    }

                    for (const auto argument : table->instantiatedTypePackParams) {
                        traverse(argument);
                    }

                    if (!is_exhaustive && (table->name || table->syntheticName)) {
                        return false;
                    }

                    for (const auto &[name, property] : table->props) {
                        budget.check();
                        payload(name.size() + 1);
                        check(property.readTy.value_or(nullptr));
                        check(property.writeTy.value_or(nullptr));
                    }

                    if (table->indexer) {
                        check(table->indexer->indexType);
                        check(table->indexer->indexResultType);
                    }
                } else if (const auto *external = Luau::get<Luau::ExternType>(value)) {
                    is_structural = false;
                    payload(external->name.size());

                    return false;
                } else if (const auto *metatable = Luau::get<Luau::MetatableType>(value)) {
                    check(metatable->table);
                    check(metatable->metatable);
                } else if (const auto *union_type = Luau::get<Luau::UnionType>(value)) {
                    is_structural = false;

                    for (const auto member : union_type->options) {
                        check(member);
                    }
                } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(value)) {
                    is_structural = false;

                    for (const auto member : intersection->parts) {
                        check(member);
                    }
                } else if (const auto *singleton = Luau::get<Luau::SingletonType>(value)) {
                    is_structural = false;

                    if (const auto *string = Luau::get<Luau::StringSingleton>(singleton)) {
                        payload(string->value.size());
                    }
                } else if (!Luau::get<Luau::PrimitiveType>(value) && !Luau::get<Luau::AnyType>(value)) {
                    is_structural = false;
                }

                return true;
            }

            bool visit(Luau::TypePackId value) override {
                count();

                if (Luau::get<Luau::BoundTypePack>(value)) {
                    return true;
                }

                if (const auto *pack = Luau::get<Luau::TypePack>(value)) {
                    if (pack->head.size() > 16384) {
                        throw Limit{};
                    }

                    for (const auto type : pack->head) {
                        check(type);
                    }
                } else if (const auto *variadic = Luau::get<Luau::VariadicTypePack>(value)) {
                    check(variadic->ty);
                } else {
                    is_structural = false;
                }

                return true;
            }

            void cycle(Luau::TypeId value) override {
                count();
                is_structural = false;

                if (Luau::get<Luau::BoundType>(value)) {
                    throw Limit{};
                }
            }

            void cycle(Luau::TypePackId) override {
                throw Limit{};
            }

            void check(Luau::TypeId value) {
                budget.check();

                if (++edges > 16384 || (value && Luau::get_if<Luau::LazyType>(&value->ty))) {
                    throw Limit{};
                }
            }

            bool is_structural = true;

        private:
            void count() {
                budget.check();

                if (++nodes > 2048 || recursionCounter > 64) {
                    throw Limit{};
                }
            }

            void payload(std::size_t size) {
                if (size > maximum_bytes - bytes) {
                    throw Limit{};
                }

                bytes += size;
            }

            PresentationBudget &budget;
            bool is_exhaustive;
            std::size_t nodes = 0;
            std::size_t edges = 0;
            std::size_t bytes = 0;
        };

        std::size_t characters(std::string_view text) {
            return std::count_if(text.begin(), text.end(), [](unsigned char character) {
                return (character & 0xC0U) != 0x80U;
            });
        }

        struct Label {
            struct Part {
                std::string text;
                char separator = 0;
                bool is_variadic = false;
            };

            char opening = 0;
            std::vector<Part> parts{{}};

            std::string render(char closing, PresentationBudget &budget) const {
                std::string text;

                for (const auto &part : parts) {
                    budget.check();

                    if (part.separator) {
                        text += part.separator;
                    }

                    text += part.text;
                }

                if (characters(text) + (opening ? 2 : 0) > maximum_label_characters) {
                    if (opening == '{') {
                        text = "…";
                    } else if (parts.size() > 1) {
                        std::string shortened;
                        bool was_omitted = false;

                        for (std::size_t index = 0; index < parts.size(); ++index) {
                            budget.check();
                            const auto &part = parts[index];
                            const bool should_retain = index == 0 || part.is_variadic;

                            if (!should_retain && was_omitted) {
                                continue;
                            }

                            if (part.separator) {
                                shortened += part.separator == ',' ? ", " : std::string{" "} + part.separator + " ";
                            }

                            if (should_retain) {
                                const auto start = part.text.find_first_not_of(" \t\r\n");

                                if (start != std::string::npos) {
                                    shortened
                                        .append(part.text, start, part.text.find_last_not_of(" \t\r\n") - start + 1);
                                }
                            } else {
                                shortened += "…";
                            }

                            was_omitted = !should_retain;
                        }

                        if (characters(shortened) < characters(text)) {
                            text = std::move(shortened);
                        }
                    }
                }

                return opening ? opening + text + closing : text;
            }
        };

        std::string unicode(const std::string &value, PresentationBudget &budget) {
            std::string result;

            for (std::size_t index = 0; index < value.size();) {
                budget.check();
                const auto byte = static_cast<unsigned char>(value[index]);
                const std::size_t length = byte < 0x80 ? 1 : byte < 0xe0 ? 2 : byte < 0xf0 ? 3 : 4;
                bool is_valid = false;

                if (length <= value.size() - index) {
                    try {
                        const rust::Str checked(value.data() + index, length);
                        result.append(checked.data(), checked.size());
                        index += length;
                        is_valid = true;
                    } catch (const std::invalid_argument &) {
                        is_valid = false;
                    }
                }

                if (!is_valid) {
                    Luau::formatAppend(result, "\\%03u", static_cast<unsigned>(byte));
                    ++index;
                }

                if (result.size() > maximum_bytes) {
                    throw Limit{};
                }
            }

            return result;
        }
    }

    PresentationBudget::PresentationBudget(std::shared_ptr<Luau::FrontendCancellationToken> cancellation)
        : cancellation(std::move(cancellation)) {}

    void PresentationBudget::check() const {
        if (cancellation && cancellation->requested()) {
            throw std::runtime_error("Analysis interrupted.");
        }
    }

    Printer::Printer(PresentationBudget &budget, Luau::ScopePtr scope) : budget(budget) {
        options.scope = std::move(scope);
        options.functionTypeArguments = true;
        options.hideTableKind = true;
    }

    std::string Printer::finish(std::string value) {
        budget.check();

        try {
            if (value.empty() || value.size() > maximum_bytes ||
                static_cast<std::size_t>(std::count(value.begin(), value.end(), '\n')) >= maximum_lines) {
                throw Limit{};
            }

            return unicode(value, budget);
        } catch (const Limit &) {
            is_complete = false;

            return "<omitted>";
        }
    }

    template <typename Formatter> std::string Printer::render(bool is_expanded, Formatter formatter) {
        auto rendering = options;
        rendering.useLineBreaks = is_expanded;
        rendering.hideTableAliasExpansions = true;

        rendering.maxTypeLength =
            rendering.maxTypeLength ? std::min(rendering.maxTypeLength, maximum_bytes) : maximum_bytes;

        auto result = formatter(rendering);
        budget.check();

        if (result.truncated || result.name.size() > rendering.maxTypeLength) {
            rendering.maxTableLength = rendering.maxTableLength
                                           ? std::min(rendering.maxTableLength, rendering.maxTypeLength)
                                           : rendering.maxTypeLength;

            rendering.maxTypeLength = maximum_bytes;
            result = formatter(rendering);
            is_complete = false;
        }

        is_complete &= !result.invalid && !result.error && !result.truncated &&
                       result.name.size() <= rendering.maxTypeLength && rendering.maxTableLength == 0;

        options.nameMap = std::move(rendering.nameMap);

        return finish(std::move(result.name));
    }

    std::string Printer::type(Luau::TypeId value, bool is_expanded) {
        try {
            if (!value) {
                throw Limit{};
            }

            Inspection inspection(budget, false);
            inspection.check(value);
            const auto followed = Luau::follow(value, Luau::FollowOption::DisableLazyTypeThunks);
            std::optional<Luau::Type> declaration;

            if (is_expanded && (Luau::get<Luau::TableType>(followed) || Luau::get<Luau::MetatableType>(followed))) {
                declaration.emplace(followed->clone());

                if (auto *table = Luau::get_if<Luau::TableType>(&declaration->ty)) {
                    table->name.reset();
                    table->syntheticName.reset();
                } else if (auto *metatable = Luau::get_if<Luau::MetatableType>(&declaration->ty)) {
                    metatable->syntheticName.reset();
                }

                value = &*declaration;
            }

            inspection.traverse(value);

            return render(is_expanded, [&](Luau::ToStringOptions &rendering) {
                return Luau::toStringDetailed(value, rendering);
            });
        } catch (const Limit &) {
            is_complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::pack(Luau::TypePackId value, bool is_expanded, bool is_return_pack) {
        try {
            if (!value) {
                throw Limit{};
            }

            Inspection inspection(budget, false);
            inspection.traverse(value);

            auto text = render(is_expanded, [&](Luau::ToStringOptions &rendering) {
                return Luau::toStringDetailed(value, rendering);
            });

            if (is_return_pack && Luau::get<Luau::TypePack>(Luau::follow(value))) {
                const auto size = Luau::size(value);

                if (Luau::finite(value) ? size > 1 : size != 0) {
                    text = "(" + text + ")";
                }
            }

            return finish(std::move(text));
        } catch (const Limit &) {
            is_complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::function(const Luau::FunctionType &value, const std::string &name, bool is_expanded) {
        try {
            Inspection inspection(budget, false);
            inspection.traverse(value.argTypes);
            inspection.traverse(value.retTypes);

            return render(is_expanded, [&](Luau::ToStringOptions &rendering) {
                rendering.hideFunctionSelfArgument = name.find(':') != std::string::npos;
                Luau::ToStringResult result;
                result.name = Luau::toStringNamedFunction(name, value, rendering);

                return result;
            });
        } catch (const Limit &) {
            is_complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::signature(Luau::TypeId value) {
        try {
            if (!value) {
                throw Limit{};
            }

            Inspection inspection(budget, false);
            inspection.check(value);
            inspection.traverse(value);
            Luau::ToStringOptions canonical;

            return finish(Luau::toStringDetailed(value, canonical).name);
        } catch (const Limit &) {
            is_complete = false;

            return "<omitted>";
        }
    }

    NativePresentation Printer::presentation(Luau::TypeId value, bool is_expanded) {
        const auto compact = type(value);
        const auto detail = is_expanded ? type(value, true) : compact;

        return {compact, detail, is_complete, false};
    }

    NativePresentation Printer::presentation(Luau::TypePackId value, bool is_expanded) {
        const auto compact = pack(value, false, true);
        const auto detail = is_expanded ? pack(value, true, true) : compact;

        return {compact, detail, is_complete, true};
    }

    void Printer::abbreviate(NativePresentation &presentation) {
        const std::string text(presentation.compact);

        if (characters(text) <= maximum_label_characters) {
            return;
        }

        Luau::Allocator allocator;
        Luau::AstNameTable names(allocator);
        Luau::Lexer lexer(text.data(), text.size(), names);
        std::vector<Label> labels(1);
        std::size_t previous = 0;

        while (lexer.next().type != Luau::Lexeme::Eof) {
            budget.check();
            const auto &token = lexer.current();
            const auto end = lexer.getOffset();
            auto start = previous;

            while (start < end && Luau::isSpace(text[start])) {
                ++start;
            }

            auto &label = labels.back();
            auto &part = label.parts.back();
            part.text.append(text, previous, start - previous);
            const char character = token.type < Luau::Lexeme::Char_END ? static_cast<char>(token.type) : 0;
            const auto opening = std::string_view("({[<").find(character);
            const auto closing = std::string_view(")}]>").find(character);

            if (opening != std::string_view::npos) {
                labels.push_back({character});
            } else if (closing != std::string_view::npos) {
                if (labels.size() == 1 || std::string_view("({[<").find(label.opening) != closing) {
                    return;
                }

                const auto nested = label.render(character, budget);
                labels.pop_back();
                labels.back().parts.back().text += nested;
            } else if (character == ',' || character == '|' || character == '&') {
                label.parts.push_back({{}, character});
            } else if (token.type == Luau::Lexeme::QuotedString &&
                       characters(std::string_view(text).substr(start, end - start)) > maximum_label_characters) {
                part.text += "…";
            } else if (token.type == Luau::Lexeme::Name && std::string_view(token.name) == "where" &&
                       labels.size() == 1 && lexer.lookahead().type == Luau::Lexeme::Name) {
                part.text += "where …";
                break;
            } else {
                if (token.type == Luau::Lexeme::Dot3) {
                    auto marker = lexer;

                    if (marker.next().type == Luau::Lexeme::Number && marker.next().type == Luau::Lexeme::Name &&
                        std::string_view(marker.current().name) == "more" && marker.next().type == Luau::Lexeme::Dot3) {
                        for (unsigned index = 0; index < 3; ++index) {
                            lexer.next();
                        }

                        part.text += "…";
                        previous = lexer.getOffset();
                        continue;
                    }

                    part.is_variadic = true;
                }

                part.text.append(text, start, end - start);
            }

            previous = end;
        }

        if (labels.size() == 1) {
            auto shortened = labels.front().render(0, budget);

            if (shortened != text) {
                presentation.compact = finish(std::move(shortened));
                presentation.is_complete = false;
            }
        }
    }

    std::size_t Printer::equivalence(Luau::TypeId value, const std::string &text, std::vector<CompletionType> &types) {
        if (!is_complete) {
            return 0;
        }

        Inspection inspection(budget, true);

        try {
            inspection.check(value);
            inspection.traverse(value);
        } catch (const Limit &) {
            return 0;
        }

        value = Luau::follow(value, Luau::FollowOption::DisableLazyTypeThunks);

        for (std::size_t index = 0; index < types.size(); ++index) {
            budget.check();
            const auto &previous = types[index];

            if (text != previous.text) {
                continue;
            }

            Luau::SeenSet seen;

            if (value == previous.type ||
                (inspection.is_structural && previous.is_structural && Luau::areEqual(seen, *value, *previous.type))) {
                return index + 1;
            }
        }

        types.push_back({value, text, inspection.is_structural});

        return types.size();
    }
}
