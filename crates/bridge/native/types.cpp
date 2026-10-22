#include "types.hpp"

#include "Luau/StringUtils.h"
#include "Luau/StructuralTypeEquality.h"
#include "Luau/VisitType.h"

#include <algorithm>
#include <exception>
#include <stdexcept>
#include <utility>

namespace instar::luau {
    namespace {
        constexpr std::size_t maximum_bytes = 32 * 1024 - 32;
        constexpr std::size_t maximum_lines = 250;

        struct Limit final : std::exception {};

        class Inspection final : public Luau::TypeVisitor {
        public:
            explicit Inspection(PresentationBudget &budget)
                : Luau::TypeVisitor("Presentation", false), budget(budget) {}

            bool visit(Luau::TypeId value) override {
                count();
                check(value);

                if (const auto *bound = Luau::get<Luau::BoundType>(value)) {
                    check(bound->boundTo);
                } else if (const auto *function = Luau::get<Luau::FunctionType>(value)) {
                    structural &= function->generics.empty() && function->genericPacks.empty() &&
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
                    structural &= !table->name && !table->syntheticName;
                    payload(table->name ? table->name->size() : 0);
                    payload(table->syntheticName ? table->syntheticName->size() : 0);

                    if (table->boundTo) {
                        check(*table->boundTo);
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

                    for (const auto argument : table->instantiatedTypeParams) {
                        check(argument);
                        traverse(argument);
                    }

                    for (const auto argument : table->instantiatedTypePackParams) {
                        traverse(argument);
                    }
                } else if (const auto *external = Luau::get<Luau::ExternType>(value)) {
                    structural = false;
                    payload(external->name.size());

                    return false;
                } else if (const auto *metatable = Luau::get<Luau::MetatableType>(value)) {
                    check(metatable->table);
                    check(metatable->metatable);
                } else if (const auto *union_type = Luau::get<Luau::UnionType>(value)) {
                    structural = false;

                    for (const auto member : union_type->options) {
                        check(member);
                    }
                } else if (const auto *intersection = Luau::get<Luau::IntersectionType>(value)) {
                    structural = false;

                    for (const auto member : intersection->parts) {
                        check(member);
                    }
                } else if (const auto *singleton = Luau::get<Luau::SingletonType>(value)) {
                    structural = false;

                    if (const auto *string = Luau::get<Luau::StringSingleton>(singleton)) {
                        payload(string->value.size());
                    }
                } else if (!Luau::get<Luau::PrimitiveType>(value) && !Luau::get<Luau::AnyType>(value)) {
                    structural = false;
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
                    structural = false;
                }

                return true;
            }

            void cycle(Luau::TypeId value) override {
                count();
                structural = false;

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

            bool structural = true;

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
            std::size_t nodes = 0;
            std::size_t edges = 0;
            std::size_t bytes = 0;
        };

        std::string unicode(const std::string &value, PresentationBudget &budget) {
            std::string result;

            for (std::size_t index = 0; index < value.size();) {
                budget.check();
                const auto byte = static_cast<unsigned char>(value[index]);
                const std::size_t length = byte < 0x80 ? 1 : byte < 0xe0 ? 2 : byte < 0xf0 ? 3 : 4;
                bool valid = false;

                if (length <= value.size() - index) {
                    try {
                        const rust::Str checked(value.data() + index, length);
                        result.append(checked.data(), checked.size());
                        index += length;
                        valid = true;
                    } catch (const std::invalid_argument &) {
                        valid = false;
                    }
                }

                if (!valid) {
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
        options.exhaustive = true;
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
            complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::type(Luau::TypeId value, bool expanded) {
        try {
            if (!value) {
                throw Limit{};
            }

            Inspection inspection(budget);
            inspection.check(value);
            inspection.traverse(value);
            options.useLineBreaks = expanded;
            options.hideTableAliasExpansions = !expanded;
            const auto result = Luau::toStringDetailed(value, options);

            complete &= !result.invalid && !result.error && !result.truncated &&
                        (options.maxTypeLength == 0 || result.name.size() <= options.maxTypeLength) &&
                        options.maxTableLength == 0;

            return finish(result.name);
        } catch (const Limit &) {
            complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::pack(Luau::TypePackId value, bool expanded, bool returns) {
        try {
            if (!value) {
                throw Limit{};
            }

            Inspection inspection(budget);
            inspection.traverse(value);
            options.useLineBreaks = expanded;
            const auto result = Luau::toStringDetailed(value, options);

            complete &= !result.invalid && !result.error && !result.truncated &&
                        (options.maxTypeLength == 0 || result.name.size() <= options.maxTypeLength) &&
                        options.maxTableLength == 0;

            auto text = result.name;

            if (returns && Luau::get<Luau::TypePack>(Luau::follow(value))) {
                const auto size = Luau::size(value);

                if (Luau::finite(value) ? size > 1 : size != 0) {
                    text = "(" + text + ")";
                }
            }

            return finish(std::move(text));
        } catch (const Limit &) {
            complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::function(const Luau::FunctionType &value, const std::string &name, bool expanded) {
        try {
            Inspection inspection(budget);
            inspection.traverse(value.argTypes);
            inspection.traverse(value.retTypes);
            options.useLineBreaks = expanded;
            options.hideFunctionSelfArgument = name.find(':') != std::string::npos;
            options.hideTableAliasExpansions = true;

            return finish(Luau::toStringNamedFunction(name, value, options));
        } catch (const Limit &) {
            complete = false;

            return "<omitted>";
        }
    }

    std::string Printer::signature(Luau::TypeId value) {
        const auto saved = options;
        options = Luau::ToStringOptions{};
        const auto text = type(value);
        options = saved;

        return text;
    }

    NativePresentation Printer::presentation(Luau::TypeId value, bool expanded) {
        const auto compact = type(value);
        const auto detail = expanded ? type(value, true) : compact;

        return {compact, detail, complete, false};
    }

    NativePresentation Printer::presentation(Luau::TypePackId value, bool expanded) {
        const auto compact = pack(value, false, true);
        const auto detail = expanded ? pack(value, true, true) : compact;

        return {compact, detail, complete, true};
    }

    std::size_t Printer::equivalence(Luau::TypeId value, const std::string &text, std::vector<CompletionType> &types) {
        if (!complete) {
            return 0;
        }

        Inspection inspection(budget);

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
                (inspection.structural && previous.structural && Luau::areEqual(seen, *value, *previous.type))) {
                return index + 1;
            }
        }

        types.push_back({value, text, inspection.structural});

        return types.size();
    }
}
