#ifndef EDITOR_HPP
#define EDITOR_HPP

#include "bridge.hpp"

#ifdef __cplusplus
#include "Luau/Frontend.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif
    /**Hover information for one source position.*/
    typedef struct EditorHover {
        /**Symbol name.*/
        Text name;
        /**Displayed type.*/
        Text type;
        /**Documentation symbol identifier.*/
        Text documentation;
        /**Whether a range is present.*/
        uint8_t has_range;
        /**Hover range when present.*/
        Location range;
    } EditorHover;

    /**Completion source mode.*/
    typedef enum EditorCompletionMode {
        /**Normal expression completion.*/
        CompletionNormal = 0,
        /**Import completion.*/
        CompletionImport = 1,
        /**Require-path completion.*/
        CompletionRequirePath = 2,
    } EditorCompletionMode;

    /**Completion item kind.*/
    typedef enum EditorCompletionKind {
        /**Plain text completion.*/
        CompletionText = 0,
        /**Method completion.*/
        CompletionMethod = 1,
        /**Function completion.*/
        CompletionFunction = 2,
        /**Constructor completion.*/
        CompletionConstructor = 3,
        /**Field completion.*/
        CompletionField = 4,
        /**Variable completion.*/
        CompletionVariable = 5,
        /**Class completion.*/
        CompletionClass = 6,
        /**Interface completion.*/
        CompletionInterface = 7,
        /**Module completion.*/
        CompletionModule = 8,
        /**Property completion.*/
        CompletionProperty = 9,
        /**Unit completion.*/
        CompletionUnit = 10,
        /**Value completion.*/
        CompletionValue = 11,
        /**Enum completion.*/
        CompletionEnum = 12,
        /**Keyword completion.*/
        CompletionKeyword = 13,
        /**Snippet completion.*/
        CompletionSnippet = 14,
        /**Color completion.*/
        CompletionColor = 15,
        /**File completion.*/
        CompletionFile = 16,
        /**Reference completion.*/
        CompletionReference = 17,
        /**Folder completion.*/
        CompletionFolder = 18,
        /**Enum-member completion.*/
        CompletionEnumMember = 19,
        /**Constant completion.*/
        CompletionConstant = 20,
        /**Struct completion.*/
        CompletionStruct = 21,
        /**Event completion.*/
        CompletionEvent = 22,
        /**Operator completion.*/
        CompletionOperator = 23,
        /**Type-parameter completion.*/
        CompletionTypeParameter = 24,
    } EditorCompletionKind;

    /**Document symbol kind.*/
    typedef enum EditorSymbolKind {
        /**File symbol.*/
        SymbolFile = 0,
        /**Module symbol.*/
        SymbolModule = 1,
        /**Namespace symbol.*/
        SymbolNamespace = 2,
        /**Package symbol.*/
        SymbolPackage = 3,
        /**Class symbol.*/
        SymbolClass = 4,
        /**Method symbol.*/
        SymbolMethod = 5,
        /**Property symbol.*/
        SymbolProperty = 6,
        /**Field symbol.*/
        SymbolField = 7,
        /**Constructor symbol.*/
        SymbolConstructor = 8,
        /**Enum symbol.*/
        SymbolEnum = 9,
        /**Interface symbol.*/
        SymbolInterface = 10,
        /**Function symbol.*/
        SymbolFunction = 11,
        /**Variable symbol.*/
        SymbolVariable = 12,
        /**Constant symbol.*/
        SymbolConstant = 13,
        /**String symbol.*/
        SymbolString = 14,
        /**Number symbol.*/
        SymbolNumber = 15,
        /**Boolean symbol.*/
        SymbolBoolean = 16,
        /**Array symbol.*/
        SymbolArray = 17,
        /**Object symbol.*/
        SymbolObject = 18,
        /**Key symbol.*/
        SymbolKey = 19,
        /**Null symbol.*/
        SymbolNull = 20,
        /**Enum-member symbol.*/
        SymbolEnumMember = 21,
        /**Struct symbol.*/
        SymbolStruct = 22,
        /**Event symbol.*/
        SymbolEvent = 23,
        /**Operator symbol.*/
        SymbolOperator = 24,
        /**Type-parameter symbol.*/
        SymbolTypeParameter = 25,
    } EditorSymbolKind;

    /**Semantic token kind.*/
    typedef enum EditorSemanticTokenKind {
        /**Variable token.*/
        SemanticTokenVariable = 0,
        /**Function token.*/
        SemanticTokenFunction = 1,
        /**Property token.*/
        SemanticTokenProperty = 2,
        /**Type token.*/
        SemanticTokenType = 3,
        /**Namespace token.*/
        SemanticTokenNamespace = 4,
        /**Parameter token.*/
        SemanticTokenParameter = 5,
        /**Method token.*/
        SemanticTokenMethod = 6,
        /**Type-parameter token.*/
        SemanticTokenTypeParameter = 7,
        /**Class token.*/
        SemanticTokenClass = 8,
    } EditorSemanticTokenKind;

    /**One completion item.*/
    typedef struct EditorCompletionItem {
        /**Completion label.*/
        Text name;
        /**Completion detail.*/
        Text detail;
        /**Documentation symbol identifier.*/
        Text documentation;
        /**Completion insertion text.*/
        Text insert;
        /**Whether a range is present.*/
        uint8_t has_range;
        /**Completion range when present.*/
        Location range;
        /**Completion source mode.*/
        EditorCompletionMode mode;
        /**Completion item kind.*/
        EditorCompletionKind kind;
        /**Whether the item is deprecated.*/
        uint8_t deprecated;
    } EditorCompletionItem;

    /**Signature help for one call site.*/
    typedef struct EditorSignatureHelp {
        /**Signature label.*/
        Text label;
        /**Signature parameter labels.*/
        const Text *parameters;
        /**Number of parameters.*/
        size_t parameter_count;
        /**Whether an active parameter is present.*/
        uint8_t has_active_parameter;
        /**Active parameter index.*/
        uint32_t active_parameter;
    } EditorSignatureHelp;

    /**Navigation target and selection range.*/
    typedef struct EditorNavigation {
        /**Target source path.*/
        Text path;
        /**Full target range.*/
        Location range;
        /**Name-selection range.*/
        Location selection;
    } EditorNavigation;

    /**One reference occurrence.*/
    typedef struct EditorReference {
        /**Reference source path.*/
        Text path;
        /**Reference range.*/
        Location range;
        /**Whether the reference is a declaration.*/
        uint8_t declaration;
    } EditorReference;

    /**One document or scope symbol.*/
    typedef struct EditorSymbol {
        /**Symbol name.*/
        Text name;
        /**Symbol source path.*/
        Text path;
        /**Full symbol range.*/
        Location range;
        /**Name-selection range.*/
        Location selection;
        /**Symbol kind.*/
        EditorSymbolKind kind;
        /**Symbol modifier bits.*/
        uint32_t modifiers;
        /**Whether the symbol is a declaration.*/
        uint8_t declaration;
    } EditorSymbol;

    /**One semantic token.*/
    typedef struct EditorSemanticToken {
        /**Token range.*/
        Location range;
        /**Token kind.*/
        EditorSemanticTokenKind kind;
        /**Token modifier bits.*/
        uint32_t modifiers;
        /**Whether the token is a declaration.*/
        uint8_t declaration;
    } EditorSemanticToken;

    /**Inferred type for one source range.*/
    typedef struct EditorTypeHint {
        /**Type-hint range.*/
        Location range;
        /**Inferred type text.*/
        Text type;
    } EditorTypeHint;

    /**Receives hover information.*/
    typedef uint8_t (*HoverCallback)(void *context, const EditorHover *hover);
    /**Receives one completion item.*/
    typedef uint8_t (*CompletionCallback)(void *context, const EditorCompletionItem *item);
    /**Receives signature help.*/
    typedef uint8_t (*SignatureCallback)(void *context, const EditorSignatureHelp *signature);
    /**Receives one navigation target.*/
    typedef uint8_t (*NavigationCallback)(void *context, const EditorNavigation *navigation);
    /**Receives one reference occurrence.*/
    typedef uint8_t (*ReferenceCallback)(void *context, const EditorReference *reference);
    /**Receives one symbol.*/
    typedef uint8_t (*SymbolCallback)(void *context, const EditorSymbol *symbol);
    /**Receives one semantic token.*/
    typedef uint8_t (*TokenCallback)(void *context, const EditorSemanticToken *token);
    /**Receives one type hint.*/
    typedef uint8_t (*HintCallback)(void *context, const EditorTypeHint *hint);

    /**Returns hover information at a source position.*/
    int32_t editor_hover(void *checker, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context, String *error);
    /**Returns completion items at a source position.*/
    int32_t editor_completion(void *checker, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context, String *error);
    /**Returns signature help at a source position.*/
    int32_t editor_signature_help(void *checker, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context, String *error);
    /**Returns inferred type hints for a module.*/
    int32_t editor_type_hints(void *checker, Text name, HintCallback callback, void *context, String *error);
    /**Returns semantic tokens for a module.*/
    int32_t editor_semantic_tokens(void *checker, Text name, TokenCallback callback, void *context, String *error);
    /**Finds the definition at a source position.*/
    int32_t editor_definition(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds the declaration at a source position.*/
    int32_t editor_declaration(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds the type definition at a source position.*/
    int32_t editor_type_definition(void *checker, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context, String *error);
    /**Finds references at a source position.*/
    int32_t editor_references(void *checker, Text name, uint32_t line, uint32_t column, ReferenceCallback callback, void *context, String *error);
    /**Prepares the symbol at a source position.*/
    int32_t editor_prepare(void *checker, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context, String *error);
    /**Finds local references at a source position.*/
    int32_t editor_local_references(void *checker, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context, String *error);
#ifdef __cplusplus
}

namespace instar {
    int32_t editor_hover(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, HoverCallback callback, void *context);
    int32_t editor_completion(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, CompletionCallback callback, void *context);
    int32_t editor_signature_help(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SignatureCallback callback, void *context);
    int32_t editor_type_hints(Luau::Frontend &frontend, Text name, HintCallback callback, void *context);
    int32_t editor_semantic_tokens(Luau::Frontend &frontend, Text name, TokenCallback callback, void *context);
    int32_t editor_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_declaration(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_type_definition(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, NavigationCallback callback, void *context);
    int32_t editor_references(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, ReferenceCallback callback, void *context);
    int32_t editor_prepare(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context);
    int32_t editor_local_references(Luau::Frontend &frontend, Text name, uint32_t line, uint32_t column, SymbolCallback callback, void *context);
} // namespace instar
#endif

#endif
