#pragma once

#include <cstddef>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace instar {

    struct Position {
        unsigned line;
        unsigned column;
    };

    struct Range {
        Position begin;
        Position end;
    };

    struct Module {
        std::string name;
        std::string source;
    };

    struct Configuration {
        std::string module;
        std::string path;
        std::string source;
    };

    struct RobloxClass {
        std::string name;
        bool service;
        bool creatable;
    };

    struct RobloxNode {
        std::string name;
        std::string class_name;
        std::optional<std::size_t> parent;
    };

    struct RobloxMetadata {
        std::vector<std::string> enumerations;
        std::vector<RobloxClass> classes;
        std::vector<RobloxNode> nodes;
    };

    using ModuleResolver = std::function<std::optional<std::string>(const std::string &, Range, std::string_view)>;

    struct Diagnostic {
        std::string path;
        std::string message;
        Range range;
        bool error;
    };

    struct Alias {
        std::string name;
        std::string value;
    };

    struct AliasResult {
        std::vector<Alias> aliases;
        std::optional<std::string> error;
    };

    struct TypeInformation {
        std::string description;
    };

    struct Completion {
        std::string label;
        std::string description;
        std::string documentation;
        std::string insert_text;
        unsigned kind;
        bool deprecated;
    };

    struct Signature {
        std::string description;
        std::vector<std::string> parameters;
        unsigned active_parameter;
    };

    struct Destination {
        std::string path;
        Range range;
        std::string name;
    };

    struct Call {
        Destination target;
        Range caller;
        std::optional<Range> container;
    };

    struct Annotation {
        std::string path;
        Position position;
        std::string text;
    };

    class Engine final {
        public:
        explicit Engine(std::vector<Module> modules = {}, ModuleResolver resolver = {},
            std::vector<Configuration> configurations = {});

        ~Engine();

        Engine(Engine &&) noexcept;
        Engine &operator=(Engine &&) noexcept;

        Engine(const Engine &) = delete;
        Engine &operator=(const Engine &) = delete;

        void update(std::vector<Module> modules, std::vector<Configuration> configurations = {});
        void load_definitions(std::vector<Module> definitions, std::optional<std::vector<std::string>> target_paths);
        void prepare_roblox(const RobloxMetadata &metadata);
        std::vector<Diagnostic> check();

        std::optional<TypeInformation> type_at(const std::string &path, Position position);
        std::optional<TypeInformation> hover(const std::string &path, Position position);
        std::vector<Completion> complete(const std::string &path, Position position);
        std::optional<Signature> signature(const std::string &path, Position position);
        std::optional<Destination> definition(const std::string &path, Position position);
        std::optional<Destination> type_definition(const std::string &path, Position position);
        std::vector<Destination> implementations(const std::string &path, Position position);
        std::vector<Destination> references(const std::string &path, Position position);
        std::vector<Annotation> annotations(const std::string &path);
        std::vector<Call> calls(const std::string &path);

        private:
        struct Implementation;
        std::unique_ptr<Implementation> implementation;
    };

    AliasResult parse_aliases(const std::string &source, bool executable);
    std::vector<Diagnostic> analyze(const std::vector<Module> &modules);

} // namespace instar
