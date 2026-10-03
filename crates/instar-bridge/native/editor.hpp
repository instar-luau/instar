#ifndef EDITOR_HPP
#define EDITOR_HPP

#include "bridge.hpp"
#include "rust/cxx.h"

#include <array>
#include <cstdint>

namespace instar {
    struct Hover;
    struct Completion;
    struct SignatureHelp;
    struct ReferenceTarget;
    struct TypeHint;
    struct Navigation;
    struct Reference;
    struct RenameTarget;
    struct Candidates;

    Failure editor_hover(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Hover> &output) noexcept;
    Failure editor_completion(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Completion> &output) noexcept;
    Failure editor_signature_help(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<SignatureHelp> &output) noexcept;
    Failure editor_type_hints(Checker &checker, Host &host, rust::Str name_input, rust::Vec<TypeHint> &output) noexcept;
    Failure editor_definition(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Navigation> &output) noexcept;
    Failure editor_declaration(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Navigation> &output) noexcept;
    Failure editor_implementation(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Navigation> &output) noexcept;
    Failure editor_type_definition(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<Navigation> &output) noexcept;
    Failure editor_references(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, Candidates candidates, rust::Vec<Reference> &output) noexcept;
    Failure editor_reference_target(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<ReferenceTarget> &output) noexcept;
    Failure editor_rename_target(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Vec<RenameTarget> &output) noexcept;
    Failure editor_rename(Checker &checker, Host &host, rust::Str name_input, std::array<uint32_t, 2> coordinates, rust::Str new_name, Candidates candidates, rust::Vec<Reference> &output) noexcept;
} // namespace instar

#endif
