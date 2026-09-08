macro_rules! define_syntax_kinds {
    ($(#[$attribute:meta])* $visibility:vis enum $enum_name:ident {$($variant:ident),* $(,)?}) => {
        $(#[$attribute])*
        $visibility enum $enum_name {
            $($variant),*
        }
        const KINDS: &[$enum_name] = &[$($enum_name::$variant),*];
    };
}

define_syntax_kinds!(
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    #[repr(u16)]
    pub enum SyntaxKind {
        Arguments,
        ArrayType,
        AssignmentStatement,
        Attribute,
        AttributeArguments,
        Attributes,
        BinaryExpression,
        Binding,
        Block,
        BreakStatement,
        CallExpression,
        ClassBase,
        ClassBody,
        ClassMethod,
        ClassProperty,
        ClassStatement,
        Comment,
        ConditionalBinding,
        ContinueStatement,
        DeclareExtern,
        DeclareFunction,
        DeclareGlobal,
        DoStatement,
        Error,
        ErrorExpression,
        ErrorStatement,
        ErrorType,
        ExportStatement,
        ExpressionList,
        ExpressionStatement,
        ExternBody,
        ExternMethod,
        ExternProperty,
        FieldExpression,
        ForStatement,
        FunctionBody,
        FunctionExpression,
        FunctionName,
        FunctionStatement,
        FunctionType,
        GenericPackParameter,
        GenericParameter,
        GenericParameters,
        GenericTypePack,
        Identifier,
        IfBranch,
        IfExpression,
        IfStatement,
        IndexExpression,
        InstantiationExpression,
        InterpolationClose,
        InterpolationEnd,
        InterpolationExpression,
        InterpolationOpen,
        InterpolationPart,
        InterpolationStart,
        InterpolationText,
        IntersectionType,
        Invalid,
        Keyword,
        LiteralExpression,
        LocalStatement,
        MethodExpression,
        Name,
        NameExpression,
        Number,
        OptionalType,
        Parameters,
        ParenthesizedExpression,
        RepeatStatement,
        ReturnStatement,
        Root,
        String,
        Symbol,
        TableExpression,
        TableField,
        TableType,
        Type,
        TypeAlias,
        TypeAnnotation,
        TypeArguments,
        TypeAssertionExpression,
        TypeDefault,
        TypeField,
        TypeFunction,
        TypeGroup,
        TypeIndexer,
        TypeName,
        TypePack,
        TypeProperty,
        TypeofType,
        UnaryExpression,
        UnionType,
        VarargExpression,
        VariadicTypePack,
        WhileStatement,
        Whitespace,
    }
);

impl SyntaxKind {
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::Comment)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Luau {}

impl rowan::Language for Luau {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        KINDS[usize::from(raw.0)]
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}
