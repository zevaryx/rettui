//! Syntax highlighting for source files, for the pages of the repositories
//! this computer hosts. A file is split into tokens by a small set of rules
//! per language (comments, strings, numbers, keywords, types, calls and
//! attributes), which is enough to read code by and cheap enough to run on
//! every page asked for. Micron (`.mu`) uses the page source view's own
//! tokenizer, so it is coloured as its markup works.
//!
//! The text is the network's, so nothing here can panic on it or take more
//! than one pass over it.

use std::ops::Range;

/// What a piece of source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    Type,
    /// `true`, `None`, `nil` and the like.
    Constant,
    String,
    Number,
    Comment,
    /// A name that is called: `name(`.
    Function,
    /// Attributes, decorators, macros, preprocessor lines, section headers.
    Meta,
    /// A diff's added and removed lines.
    Inserted,
    Deleted,
}

/// How a language marks its strings.
#[derive(Debug, Clone, Copy)]
struct Quote {
    open: &'static str,
    close: &'static str,
    /// Backslash escapes the next character.
    escapes: bool,
    /// It may go on past the end of a line.
    lines: bool,
}

const fn quote(open: &'static str, close: &'static str, escapes: bool, lines: bool) -> Quote {
    Quote { open, close, escapes, lines }
}

const DOUBLE: Quote = quote("\"", "\"", true, false);
const SINGLE: Quote = quote("'", "'", true, false);
const BACKTICK: Quote = quote("`", "`", true, true);

/// Lines that mean something by how they start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lines {
    Code,
    /// `+`, `-` and `@@` lines of a diff.
    Diff,
    /// Markdown: headings, quotes, fences.
    Markdown,
}

/// Something that marks attributes or the like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meta {
    None,
    /// `#[...]` and `#![...]`, and `name!` macros.
    Rust,
    /// A `#` line: `#include`, `#define`.
    Preprocessor,
    /// `@name`.
    At,
}

/// One language's rules.
#[derive(Debug)]
pub struct Lang {
    pub name: &'static str,
    line_comments: &'static [&'static str],
    block_comments: &'static [(&'static str, &'static str)],
    quotes: &'static [Quote],
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    constants: &'static [&'static str],
    /// Keywords match whatever their case (SQL, Dockerfile).
    any_case: bool,
    /// A name that starts with a capital is a type.
    capital_types: bool,
    meta: Meta,
    /// `'` starts a character only when it closes soon (Rust's lifetimes are `'a`).
    char_quotes: bool,
    /// `$name` and `${...}` are variables (shell, make).
    dollar_vars: bool,
    /// `key =` or `key:` at the start of a line names a key (config files).
    keys: Option<char>,
    /// `[section]` lines (INI, TOML).
    sections: bool,
    /// `<tags attr="...">` (HTML, XML).
    markup: bool,
    lines: Lines,
}

const BASE: Lang = Lang {
    name: "",
    line_comments: &[],
    block_comments: &[],
    quotes: &[DOUBLE, SINGLE],
    keywords: &[],
    types: &[],
    constants: &[],
    any_case: false,
    capital_types: false,
    meta: Meta::None,
    char_quotes: false,
    dollar_vars: false,
    keys: None,
    sections: false,
    markup: false,
    lines: Lines::Code,
};

const C_COMMENTS: &[(&str, &str)] = &[("/*", "*/")];

static RUST: Lang = Lang {
    name: "Rust",
    line_comments: &["//"],
    block_comments: C_COMMENTS,
    quotes: &[
        quote("r#\"", "\"#", false, true),
        quote("r\"", "\"", false, true),
        quote("b\"", "\"", true, true),
        quote("\"", "\"", true, true),
        SINGLE,
    ],
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for", "if", "impl", "in",
        "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "super", "trait", "type", "unsafe",
        "use", "where", "while", "yield",
    ],
    types: &[
        "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "Self",
    ],
    constants: &["true", "false", "self", "None", "Some", "Ok", "Err"],
    capital_types: true,
    meta: Meta::Rust,
    char_quotes: true,
    ..BASE
};

static PYTHON: Lang = Lang {
    name: "Python",
    line_comments: &["#"],
    quotes: &[quote("\"\"\"", "\"\"\"", true, true), quote("'''", "'''", true, true), DOUBLE, SINGLE],
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for",
        "from", "global", "if", "import", "in", "is", "lambda", "match", "case", "nonlocal", "not", "or", "pass", "raise", "return", "try",
        "while", "with", "yield",
    ],
    types: &["int", "float", "str", "bytes", "bool", "list", "dict", "set", "tuple", "object"],
    constants: &["True", "False", "None", "self", "cls"],
    capital_types: true,
    meta: Meta::At,
    ..BASE
};

const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline", "register",
    "restrict", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while",
];
const C_TYPES: &[&str] = &[
    "char", "double", "float", "int", "long", "short", "signed", "unsigned", "void", "bool", "size_t", "ssize_t", "int8_t", "int16_t",
    "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
];

static C: Lang = Lang {
    name: "C",
    line_comments: &["//"],
    block_comments: C_COMMENTS,
    keywords: C_KEYWORDS,
    types: C_TYPES,
    constants: &["NULL", "true", "false"],
    meta: Meta::Preprocessor,
    ..BASE
};

static CPP: Lang = Lang {
    name: "C++",
    keywords: &[
        "auto",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "constexpr",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "explicit",
        "extern",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "namespace",
        "new",
        "noexcept",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "return",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "try",
        "typedef",
        "typename",
        "union",
        "using",
        "virtual",
        "volatile",
        "while",
    ],
    constants: &["nullptr", "NULL", "true", "false"],
    capital_types: true,
    ..C
};

static GO: Lang = Lang {
    name: "Go",
    line_comments: &["//"],
    block_comments: C_COMMENTS,
    quotes: &[DOUBLE, SINGLE, quote("`", "`", false, true)],
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ],
    types: &[
        "bool",
        "byte",
        "complex64",
        "complex128",
        "error",
        "float32",
        "float64",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "rune",
        "string",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "any",
    ],
    constants: &["true", "false", "nil", "iota"],
    capital_types: false,
    ..BASE
};

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

static JAVASCRIPT: Lang = Lang {
    name: "JavaScript",
    line_comments: &["//"],
    block_comments: C_COMMENTS,
    quotes: &[DOUBLE, SINGLE, BACKTICK],
    keywords: JS_KEYWORDS,
    constants: &["true", "false", "null", "undefined", "this", "NaN", "Infinity"],
    capital_types: true,
    meta: Meta::At,
    ..BASE
};

static TYPESCRIPT: Lang = Lang {
    name: "TypeScript",
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "is",
        "keyof",
        "let",
        "namespace",
        "new",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "static",
        "super",
        "switch",
        "throw",
        "try",
        "type",
        "typeof",
        "var",
        "void",
        "while",
        "yield",
    ],
    types: &["string", "number", "boolean", "any", "unknown", "never", "object", "bigint", "symbol"],
    ..JAVASCRIPT
};

static JAVA: Lang = Lang {
    name: "Java",
    line_comments: &["//"],
    block_comments: C_COMMENTS,
    keywords: &[
        "abstract",
        "assert",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "do",
        "else",
        "enum",
        "extends",
        "final",
        "finally",
        "for",
        "if",
        "implements",
        "import",
        "instanceof",
        "interface",
        "native",
        "new",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "static",
        "super",
        "switch",
        "synchronized",
        "throw",
        "throws",
        "try",
        "var",
        "volatile",
        "while",
        "record",
        "sealed",
    ],
    types: &["boolean", "byte", "char", "double", "float", "int", "long", "short", "void"],
    constants: &["true", "false", "null", "this"],
    capital_types: true,
    meta: Meta::At,
    ..BASE
};

static KOTLIN: Lang = Lang {
    name: "Kotlin",
    quotes: &[quote("\"\"\"", "\"\"\"", false, true), DOUBLE, SINGLE],
    keywords: &[
        "as",
        "break",
        "class",
        "companion",
        "continue",
        "data",
        "do",
        "else",
        "enum",
        "for",
        "fun",
        "if",
        "import",
        "in",
        "interface",
        "internal",
        "is",
        "object",
        "open",
        "override",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "sealed",
        "suspend",
        "throw",
        "try",
        "catch",
        "finally",
        "val",
        "var",
        "when",
        "while",
    ],
    types: &[],
    ..JAVA
};

static CSHARP: Lang = Lang {
    name: "C#",
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "base",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delegate",
        "do",
        "else",
        "enum",
        "event",
        "explicit",
        "extern",
        "finally",
        "fixed",
        "for",
        "foreach",
        "if",
        "implicit",
        "in",
        "interface",
        "internal",
        "is",
        "lock",
        "namespace",
        "new",
        "operator",
        "out",
        "override",
        "params",
        "private",
        "protected",
        "public",
        "readonly",
        "ref",
        "return",
        "sealed",
        "sizeof",
        "static",
        "struct",
        "switch",
        "throw",
        "try",
        "typeof",
        "using",
        "var",
        "virtual",
        "void",
        "while",
    ],
    types: &["bool", "byte", "char", "decimal", "double", "float", "int", "long", "object", "sbyte", "short", "string", "uint", "ulong"],
    meta: Meta::None,
    ..JAVA
};

static SWIFT: Lang = Lang {
    name: "Swift",
    quotes: &[quote("\"\"\"", "\"\"\"", true, true), DOUBLE],
    keywords: &[
        "as",
        "break",
        "case",
        "catch",
        "class",
        "continue",
        "default",
        "defer",
        "do",
        "else",
        "enum",
        "extension",
        "fileprivate",
        "for",
        "func",
        "guard",
        "if",
        "import",
        "in",
        "init",
        "inout",
        "internal",
        "is",
        "let",
        "private",
        "protocol",
        "public",
        "repeat",
        "return",
        "static",
        "struct",
        "subscript",
        "switch",
        "throw",
        "throws",
        "try",
        "var",
        "where",
        "while",
    ],
    types: &[],
    constants: &["true", "false", "nil", "self", "Self"],
    ..JAVA
};

static ZIG: Lang = Lang {
    name: "Zig",
    line_comments: &["//"],
    quotes: &[DOUBLE, SINGLE],
    keywords: &[
        "align",
        "and",
        "break",
        "catch",
        "comptime",
        "const",
        "continue",
        "defer",
        "else",
        "enum",
        "errdefer",
        "error",
        "export",
        "extern",
        "fn",
        "for",
        "if",
        "inline",
        "orelse",
        "or",
        "packed",
        "pub",
        "return",
        "struct",
        "switch",
        "test",
        "try",
        "union",
        "unreachable",
        "var",
        "while",
    ],
    types: &["bool", "f32", "f64", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "isize", "void", "anytype", "type"],
    constants: &["true", "false", "null", "undefined"],
    capital_types: true,
    meta: Meta::At,
    ..BASE
};

static SHELL: Lang = Lang {
    name: "Shell",
    line_comments: &["#"],
    quotes: &[quote("\"", "\"", true, true), quote("'", "'", false, true)],
    keywords: &[
        "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in", "function", "return", "local",
        "export", "readonly", "set", "unset", "shift", "exit", "source", "echo", "cd", "test",
    ],
    constants: &["true", "false"],
    dollar_vars: true,
    ..BASE
};

static LUA: Lang = Lang {
    name: "Lua",
    line_comments: &["--"],
    block_comments: &[("--[[", "]]")],
    quotes: &[quote("[[", "]]", false, true), DOUBLE, SINGLE],
    keywords: &[
        "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local", "not", "or", "repeat", "return",
        "then", "until", "while",
    ],
    constants: &["true", "false", "nil", "self"],
    ..BASE
};

static RUBY: Lang = Lang {
    name: "Ruby",
    line_comments: &["#"],
    block_comments: &[("=begin", "=end")],
    keywords: &[
        "alias",
        "and",
        "begin",
        "break",
        "case",
        "class",
        "def",
        "defined?",
        "do",
        "else",
        "elsif",
        "end",
        "ensure",
        "for",
        "if",
        "in",
        "module",
        "next",
        "not",
        "or",
        "redo",
        "rescue",
        "retry",
        "return",
        "super",
        "then",
        "undef",
        "unless",
        "until",
        "when",
        "while",
        "yield",
        "require",
        "attr_reader",
        "attr_accessor",
    ],
    constants: &["true", "false", "nil", "self"],
    capital_types: true,
    ..BASE
};

static SQL: Lang = Lang {
    name: "SQL",
    line_comments: &["--"],
    block_comments: C_COMMENTS,
    quotes: &[quote("'", "'", false, true), DOUBLE],
    keywords: &[
        "select",
        "from",
        "where",
        "and",
        "or",
        "not",
        "insert",
        "into",
        "values",
        "update",
        "set",
        "delete",
        "create",
        "table",
        "index",
        "view",
        "drop",
        "alter",
        "add",
        "primary",
        "key",
        "foreign",
        "references",
        "join",
        "left",
        "right",
        "inner",
        "outer",
        "on",
        "group",
        "by",
        "order",
        "having",
        "limit",
        "offset",
        "as",
        "distinct",
        "union",
        "all",
        "case",
        "when",
        "then",
        "else",
        "end",
        "is",
        "in",
        "like",
        "exists",
        "default",
        "unique",
        "begin",
        "commit",
        "rollback",
        "transaction",
        "if",
        "with",
    ],
    types: &["integer", "int", "text", "varchar", "char", "blob", "real", "boolean", "date", "timestamp", "numeric", "float", "bigint"],
    constants: &["null", "true", "false"],
    any_case: true,
    ..BASE
};

static TOML: Lang = Lang {
    name: "TOML",
    line_comments: &["#"],
    quotes: &[quote("\"\"\"", "\"\"\"", true, true), quote("'''", "'''", false, true), DOUBLE, quote("'", "'", false, false)],
    constants: &["true", "false"],
    keys: Some('='),
    sections: true,
    ..BASE
};

static INI: Lang = Lang { name: "INI", line_comments: &["#", ";"], quotes: &[DOUBLE], constants: &["true", "false", "yes", "no"], ..TOML };

static YAML: Lang =
    Lang { name: "YAML", line_comments: &["#"], constants: &["true", "false", "null", "yes", "no", "~"], keys: Some(':'), ..BASE };

static JSON: Lang = Lang { name: "JSON", quotes: &[DOUBLE], constants: &["true", "false", "null"], keys: None, ..BASE };

static MARKUP: Lang = Lang { name: "HTML", block_comments: &[("<!--", "-->")], quotes: &[], markup: true, ..BASE };
static XML: Lang = Lang { name: "XML", ..MARKUP };

static CSS: Lang = Lang {
    name: "CSS",
    line_comments: &[],
    block_comments: C_COMMENTS,
    keywords: &["important", "media", "import", "keyframes", "font-face", "supports"],
    meta: Meta::At,
    keys: Some(':'),
    ..BASE
};

static MAKE: Lang = Lang {
    name: "Makefile",
    line_comments: &["#"],
    keywords: &["ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "include", "define", "endef", "export", "override", ".PHONY"],
    dollar_vars: true,
    keys: Some(':'),
    ..BASE
};

static DOCKERFILE: Lang = Lang {
    name: "Dockerfile",
    line_comments: &["#"],
    keywords: &[
        "from",
        "run",
        "cmd",
        "label",
        "expose",
        "env",
        "add",
        "copy",
        "entrypoint",
        "volume",
        "user",
        "workdir",
        "arg",
        "onbuild",
        "stopsignal",
        "healthcheck",
        "shell",
        "as",
    ],
    any_case: true,
    dollar_vars: true,
    ..BASE
};

static DIFF: Lang = Lang { name: "Diff", quotes: &[], lines: Lines::Diff, ..BASE };

static MARKDOWN: Lang = Lang { name: "Markdown", quotes: &[], lines: Lines::Markdown, ..BASE };

/// Micron, which [`highlight`] leaves to the page source tokenizer.
pub static MICRON: Lang = Lang { name: "Micron", quotes: &[], ..BASE };

/// The language of a file, by its name (and its first line, for scripts
/// that start `#!`). `None` is plain text.
pub fn language(path: &str, first_line: &str) -> Option<&'static Lang> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => Some(&MAKE),
        "dockerfile" | "containerfile" => Some(&DOCKERFILE),
        "cargo.lock" | "pipfile" => Some(&TOML),
        ".gitignore" | ".gitattributes" | ".dockerignore" => None,
        ".bashrc" | ".profile" | ".zshrc" | "pkgbuild" => Some(&SHELL),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let by_ext = match ext {
        "rs" => Some(&RUST),
        "py" | "pyw" | "pyi" => Some(&PYTHON),
        "c" | "h" => Some(&C),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ino" => Some(&CPP),
        "go" => Some(&GO),
        "js" | "mjs" | "cjs" | "jsx" => Some(&JAVASCRIPT),
        "ts" | "tsx" | "mts" => Some(&TYPESCRIPT),
        "java" => Some(&JAVA),
        "kt" | "kts" => Some(&KOTLIN),
        "cs" => Some(&CSHARP),
        "swift" => Some(&SWIFT),
        "zig" => Some(&ZIG),
        "sh" | "bash" | "zsh" | "ksh" => Some(&SHELL),
        "lua" => Some(&LUA),
        "rb" => Some(&RUBY),
        "sql" => Some(&SQL),
        "toml" => Some(&TOML),
        "ini" | "cfg" | "conf" | "service" | "desktop" => Some(&INI),
        "yml" | "yaml" => Some(&YAML),
        "json" | "jsonc" => Some(&JSON),
        "html" | "htm" | "xhtml" => Some(&MARKUP),
        "xml" | "svg" | "plist" | "xsd" => Some(&XML),
        "css" | "scss" | "less" => Some(&CSS),
        "mk" => Some(&MAKE),
        "dockerfile" => Some(&DOCKERFILE),
        "diff" | "patch" => Some(&DIFF),
        "md" | "markdown" => Some(&MARKDOWN),
        "mu" => Some(&MICRON),
        _ => None,
    };
    if by_ext.is_some() {
        return by_ext;
    }
    let shebang = first_line.strip_prefix("#!")?;
    let program = shebang.split_whitespace().flat_map(|word| word.rsplit('/').next()).find(|word| *word != "env")?;
    match program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => Some(&SHELL),
        "python" => Some(&PYTHON),
        "node" => Some(&JAVASCRIPT),
        "ruby" => Some(&RUBY),
        "lua" => Some(&LUA),
        _ => None,
    }
}

/// Spans of one line: what each is, and where it is in the line.
pub type Spans = Vec<(Token, Range<usize>)>;

/// `text` as tokens, a line at a time (split at `\n`; a `\r` before it
/// is left out).
pub fn highlight(lang: &Lang, text: &str) -> Vec<Spans> {
    let mut all = Vec::new();
    match lang.lines {
        Lines::Code => Tokenizer { lang, text, at: 0, out: &mut all, in_tag: false, blank: true }.run(),
        Lines::Diff => diff(text, &mut all),
        Lines::Markdown => markdown(text, &mut all),
    }
    split_lines(text, &all)
}

/// Tokens over the whole text, cut where its lines end.
fn split_lines(text: &str, tokens: &[(Token, Range<usize>)]) -> Vec<Spans> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut next = tokens.iter().peekable();
    let mut carried: Option<(Token, Range<usize>)> = None;
    for line in text.split('\n') {
        let end = start + line.len();
        let shown = end - usize::from(line.ends_with('\r'));
        let mut spans = Spans::new();
        loop {
            let (token, range) = match carried.take() {
                Some(piece) => piece,
                None => match next.next() {
                    Some(piece) => piece.clone(),
                    None => break,
                },
            };
            if range.start > end {
                carried = Some((token, range));
                break;
            }
            let (from, to) = (range.start.max(start), range.end.min(shown));
            if from < to && token != Token::Plain {
                spans.push((token, from - start..to - start));
            }
            if range.end > end + 1 {
                carried = Some((token, end + 1..range.end));
                break;
            }
        }
        lines.push(spans);
        start = end + 1;
    }
    lines
}

struct Tokenizer<'a> {
    lang: &'a Lang,
    text: &'a str,
    at: usize,
    out: &'a mut Vec<(Token, Range<usize>)>,
    /// Inside a markup tag, between `<name` and `>`.
    in_tag: bool,
    /// Only spaces and tabs since the line began.
    blank: bool,
}

/// How far ahead a closing mark is looked for, where a missing one would
/// otherwise have the rest of the text searched again and again.
const NEAR: usize = 256;

/// Where in `text` (within [`NEAR`] bytes) a character `found` is.
fn near(text: &str, found: impl Fn(char) -> bool) -> Option<usize> {
    text.char_indices().take_while(|(i, _)| *i < NEAR).find(|(_, c)| found(*c)).map(|(i, _)| i)
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Tokenizer<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn push(&mut self, token: Token, end: usize) {
        let end = end.min(self.text.len());
        if end <= self.at {
            return;
        }
        let piece = &self.text[self.at..end];
        let indent = |text: &str| text.chars().all(|c| c == ' ' || c == '\t');
        self.blank = match piece.rfind('\n') {
            Some(n) => indent(&piece[n + 1..]),
            None => self.blank && indent(piece),
        };
        if token != Token::Plain {
            self.out.push((token, self.at..end));
        }
        self.at = end;
    }

    /// Where the line the tokenizer is on ends.
    fn line_end(&self) -> usize {
        self.rest().find('\n').map_or(self.text.len(), |n| self.at + n)
    }

    /// Whether the tokenizer is at the start of a line, past its indent.
    fn line_start(&self) -> bool {
        self.blank
    }

    fn previous(&self) -> Option<char> {
        self.text[..self.at].chars().next_back()
    }

    fn run(&mut self) {
        while self.at < self.text.len() {
            self.step();
        }
    }

    fn step(&mut self) {
        let lang = self.lang;
        let rest = self.rest();
        let Some(c) = rest.chars().next() else { return };
        for (open, close) in lang.block_comments {
            if let Some(inside) = rest.strip_prefix(open) {
                let end = inside.find(close).map_or(self.text.len(), |n| self.at + open.len() + n + close.len());
                return self.push(Token::Comment, end);
            }
        }
        if lang.line_comments.iter().any(|lead| rest.starts_with(lead)) && !self.preprocessor(rest) {
            let end = self.line_end();
            return self.push(Token::Comment, end);
        }
        if lang.markup {
            return self.markup(rest, c);
        }
        if lang.sections && c == '[' && self.line_start() {
            let end = self.line_end();
            let end = self.text[self.at..end].find(']').map_or(end, |n| self.at + n + 1);
            return self.push(Token::Meta, end);
        }
        if self.preprocessor(rest) {
            let end = self.line_end();
            return self.push(Token::Meta, end);
        }
        if lang.meta == Meta::Rust && (rest.starts_with("#[") || rest.starts_with("#![")) {
            let end = self.line_end();
            let end = self.text[self.at..end].rfind(']').map_or(end, |n| self.at + n + 1);
            return self.push(Token::Meta, end);
        }
        if lang.meta == Meta::At && c == '@' && rest[1..].starts_with(|c: char| c.is_alphabetic()) {
            let end = self.at + 1 + rest[1..].find(|c: char| !(is_ident(c) || c == '.' || c == '-')).unwrap_or(rest.len() - 1);
            return self.push(Token::Meta, end);
        }
        if lang.dollar_vars && c == '$' {
            let end = match rest[1..].chars().next() {
                Some('{' | '(') => near(rest, |c| c == '}' || c == ')' || c == '\n').map_or(self.at + 2, |n| self.at + n + 1),
                Some(c) if is_ident(c) => self.at + 1 + rest[1..].find(|c: char| !is_ident(c)).unwrap_or(rest.len() - 1),
                Some(c) if "@#?$!*-0123456789".contains(c) => self.at + 1 + c.len_utf8(),
                _ => self.at + 1,
            };
            return self.push(Token::Type, end);
        }
        if let Some(quote) = lang.quotes.iter().find(|q| rest.starts_with(q.open) && self.opens(q)) {
            return self.string(quote);
        }
        if c.is_ascii_digit() && !self.previous().is_some_and(is_ident) {
            return self.number(rest);
        }
        if is_ident(c) || (c == '.' && lang.keywords.iter().any(|k| k.starts_with('.'))) {
            return self.word(rest);
        }
        self.push(Token::Plain, self.at + c.len_utf8());
    }

    /// A `#` line in a language with a preprocessor.
    fn preprocessor(&self, rest: &str) -> bool {
        self.lang.meta == Meta::Preprocessor && rest.starts_with('#') && self.line_start()
    }

    /// Whether a quote here starts a string: a raw or byte string's letter
    /// isn't the end of a name, and Rust's `'` is a lifetime unless it closes soon.
    fn opens(&self, quote: &Quote) -> bool {
        if quote.open.len() > 1 && quote.open.starts_with(char::is_alphabetic) && self.previous().is_some_and(is_ident) {
            return false;
        }
        if self.lang.char_quotes && quote.open == "'" {
            let rest = &self.rest()[1..];
            let mut chars = rest.chars();
            return match chars.next() {
                Some('\\') => rest.chars().take(12).skip(1).any(|c| c == '\''),
                Some(_) => chars.next() == Some('\''),
                None => false,
            };
        }
        true
    }

    fn string(&mut self, quote: &Quote) {
        let start = self.at + quote.open.len();
        let mut chars = self.text[start..].char_indices();
        let mut end = self.text.len();
        while let Some((i, c)) = chars.next() {
            if quote.escapes && c == '\\' {
                chars.next();
                continue;
            }
            if c == '\n' && !quote.lines {
                end = start + i;
                break;
            }
            if self.text[start + i..].starts_with(quote.close) {
                end = start + i + quote.close.len();
                break;
            }
        }
        self.push(Token::String, end);
    }

    fn number(&mut self, rest: &str) {
        let mut end = 0;
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let decimal = c == '.' && chars.peek().is_some_and(|(_, next)| next.is_ascii_digit());
            if !(is_ident(c) || decimal) {
                break;
            }
            end = i + c.len_utf8();
        }
        self.push(Token::Number, self.at + end.max(1));
    }

    fn word(&mut self, rest: &str) {
        let lang = self.lang;
        let len = rest
            .find(|c: char| !(is_ident(c) || c == '-' && lang.keys.is_some() || c == '.' && lang.name == "Makefile"))
            .unwrap_or(rest.len());
        let len = len.max(rest.chars().next().map_or(1, char::len_utf8));
        let word = &rest[..len];
        let after = rest[len..].trim_start_matches([' ', '\t']);
        let is = |list: &[&str]| {
            if lang.any_case { list.iter().any(|k| k.eq_ignore_ascii_case(word)) } else { list.contains(&word) }
        };
        let token = if lang.keys.is_some_and(|sep| after.starts_with(sep)) && self.line_start() {
            Token::Type
        } else if is(lang.keywords) {
            Token::Keyword
        } else if is(lang.constants) {
            Token::Constant
        } else if is(lang.types) {
            Token::Type
        } else if lang.meta == Meta::Rust && rest[len..].starts_with('!') && !rest[len..].starts_with("!=") {
            self.push(Token::Meta, self.at + len + 1);
            return;
        } else if after.starts_with('(') && lang.keys.is_none() {
            Token::Function
        } else if lang.capital_types && word.starts_with(|c: char| c.is_uppercase()) {
            Token::Type
        } else {
            Token::Plain
        };
        self.push(token, self.at + len);
    }

    /// HTML and XML: tag names, attributes and their values.
    fn markup(&mut self, rest: &str, c: char) {
        if !self.in_tag {
            if c == '<' && rest[1..].starts_with(|c: char| c.is_alphabetic() || c == '/' || c == '!' || c == '?') {
                let len = 1 + rest[1..].find(|c: char| !(is_ident(c) || "/!?:-.".contains(c))).unwrap_or(rest.len() - 1);
                self.in_tag = true;
                return self.push(Token::Keyword, self.at + len);
            }
            if c == '&' {
                let end = near(rest, |c| c == ';').filter(|n| *n < 12).map_or(self.at + 1, |n| self.at + n + 1);
                return self.push(Token::Constant, end);
            }
            let end = rest.find(['<', '&']).map_or(self.text.len(), |n| self.at + n.max(c.len_utf8()));
            return self.push(Token::Plain, end);
        }
        match c {
            '>' => {
                self.in_tag = false;
                self.push(Token::Keyword, self.at + 1);
            }
            '/' | '?' if rest[1..].starts_with('>') => {
                self.in_tag = false;
                self.push(Token::Keyword, self.at + 2);
            }
            '"' | '\'' => {
                let end = rest[1..].find(c).map_or(self.text.len(), |n| self.at + n + 2);
                self.push(Token::String, end);
            }
            c if is_ident(c) => {
                let len = rest.find(|c: char| !(is_ident(c) || ":-.".contains(c))).unwrap_or(rest.len());
                self.push(Token::Type, self.at + len);
            }
            _ => self.push(Token::Plain, self.at + c.len_utf8()),
        }
    }
}

/// A token for each whole line, by `kind`.
fn by_line(text: &str, out: &mut Vec<(Token, Range<usize>)>, mut kind: impl FnMut(&str) -> Token) {
    let mut start = 0;
    for line in text.split('\n') {
        out.push((kind(line), start..start + line.len()));
        start += line.len() + 1;
    }
}

fn diff(text: &str, out: &mut Vec<(Token, Range<usize>)>) {
    by_line(text, out, |line| match line.chars().next() {
        _ if line.starts_with("+++") || line.starts_with("---") => Token::Keyword,
        Some('+') => Token::Inserted,
        Some('-') => Token::Deleted,
        _ if line.starts_with("@@") => Token::Meta,
        _ if line.starts_with("diff ") || line.starts_with("index ") => Token::Keyword,
        _ => Token::Plain,
    });
}

fn markdown(text: &str, out: &mut Vec<(Token, Range<usize>)>) {
    let mut fence: Option<&str> = None;
    let mut start = 0;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let range = start..start + line.len();
        start += line.len() + 1;
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        if let Some(open) = fence {
            if marker == Some(open) {
                fence = None;
            }
            out.push((Token::String, range));
            continue;
        }
        if let Some(marker) = marker {
            fence = Some(marker);
            out.push((Token::String, range));
        } else if trimmed.starts_with('#') {
            out.push((Token::Keyword, range));
        } else if trimmed.starts_with('>') {
            out.push((Token::Comment, range));
        } else if line.starts_with("    ") || line.starts_with('\t') {
            out.push((Token::String, range));
        } else {
            // Inline code.
            let mut at = 0;
            while let Some(open) = line[at..].find('`') {
                let open = at + open;
                let Some(close) = line[open + 1..].find('`') else { break };
                let close = open + 1 + close + 1;
                out.push((Token::String, range.start + open..range.start + close));
                at = close;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each line's tokens, as the text they cover.
    fn tokens<'a>(lang: &Lang, text: &'a str) -> Vec<Vec<(Token, &'a str)>> {
        let lines: Vec<&str> = text.split('\n').collect();
        highlight(lang, text)
            .into_iter()
            .zip(lines)
            .map(|(spans, line)| spans.into_iter().map(|(token, range)| (token, &line[range])).collect())
            .collect()
    }

    fn has(lang: &Lang, text: &str, token: Token, what: &str) -> bool {
        tokens(lang, text).iter().flatten().any(|(t, s)| *t == token && *s == what)
    }

    #[test]
    fn languages_are_found_by_name_and_first_line() {
        let name = |path: &str, first: &str| language(path, first).map(|l| l.name);
        assert_eq!(name("src/main.rs", ""), Some("Rust"));
        assert_eq!(name("a/b/Makefile", ""), Some("Makefile"));
        assert_eq!(name("Dockerfile", ""), Some("Dockerfile"));
        assert_eq!(name("x.PY", ""), Some("Python"));
        assert_eq!(name("pages/index.mu", ""), Some("Micron"));
        assert_eq!(name("bin/tool", "#!/usr/bin/env python3"), Some("Python"));
        assert_eq!(name("run", "#!/bin/bash -e"), Some("Shell"));
        assert_eq!(name("notes.txt", ""), None);
        assert_eq!(name("noext", "#!"), None);
        assert_eq!(name("", ""), None);
    }

    #[test]
    fn rust_is_coloured_by_what_each_part_is() {
        let code = "#[derive(Debug)]\npub fn main() -> Result<u8, E> {\n    let s = \"a \\\"q\\\" b\"; // done\n    println!(\"{}\", 0x1F + 1.5);\n    let c = 'x'; fn f<'a>(_: &'a str) {}\n}";
        let t = tokens(&RUST, code);
        assert_eq!(t[0], [(Token::Meta, "#[derive(Debug)]")]);
        assert!(t[1].contains(&(Token::Keyword, "pub")) && t[1].contains(&(Token::Function, "main")));
        assert!(t[1].contains(&(Token::Type, "Result")) && t[1].contains(&(Token::Type, "u8")));
        assert!(t[2].contains(&(Token::String, "\"a \\\"q\\\" b\"")) && t[2].contains(&(Token::Comment, "// done")));
        assert!(
            t[3].contains(&(Token::Meta, "println!")) && t[3].contains(&(Token::Number, "0x1F")) && t[3].contains(&(Token::Number, "1.5"))
        );
        // A character is a string; a lifetime isn't.
        assert!(t[4].contains(&(Token::String, "'x'")));
        assert!(!t[4].iter().any(|(token, s)| *token == Token::String && s.contains("'a")));
    }

    #[test]
    fn comments_and_strings_go_on_over_lines() {
        let t = tokens(&C, "/* one\ntwo */ int x;\n#include <stdio.h>\nchar *s = \"open");
        assert_eq!(t[0], [(Token::Comment, "/* one")]);
        assert_eq!(t[1][0], (Token::Comment, "two */"));
        assert!(t[1].contains(&(Token::Type, "int")));
        assert_eq!(t[2], [(Token::Meta, "#include <stdio.h>")]);
        // An unclosed string stops at its line's end.
        assert_eq!(t[3].last(), Some(&(Token::String, "\"open")));
        let t = tokens(&PYTHON, "def f():\n    \"\"\"Doc\n    more\"\"\"\n    return None  # no");
        assert_eq!(t[1], [(Token::String, "\"\"\"Doc")]);
        assert_eq!(t[2], [(Token::String, "    more\"\"\"")]);
        assert!(t[3].contains(&(Token::Constant, "None")) && t[3].contains(&(Token::Comment, "# no")));
    }

    #[test]
    fn config_files_markup_and_scripts() {
        assert!(has(&TOML, "[package]\nname = \"rettui\" # it", Token::Meta, "[package]"));
        assert!(has(&TOML, "[package]\nname = \"rettui\"", Token::Type, "name"));
        assert!(has(&YAML, "on:\n  push: true", Token::Type, "push"));
        assert!(has(&YAML, "on:\n  push: true", Token::Constant, "true"));
        assert!(has(&JSON, "{\"a\": [1, null]}", Token::String, "\"a\""));
        let html = "<!-- c -->\n<a href=\"x\">link &amp; more</a>";
        assert!(has(&MARKUP, html, Token::Comment, "<!-- c -->"));
        assert!(has(&MARKUP, html, Token::Keyword, "<a") && has(&MARKUP, html, Token::Type, "href"));
        assert!(has(&MARKUP, html, Token::String, "\"x\"") && has(&MARKUP, html, Token::Constant, "&amp;"));
        assert!(has(&MARKUP, html, Token::Keyword, "</a"));
        assert!(has(&SHELL, "echo \"$HOME\" ${X} $1", Token::Type, "${X}"));
        assert!(has(&SHELL, "echo $1", Token::Type, "$1"));
        assert!(has(&DOCKERFILE, "FROM rust AS build", Token::Keyword, "FROM"));
        assert!(has(&SQL, "SELECT * FROM t WHERE a = 'x'", Token::Keyword, "WHERE"));
        assert!(has(&CSS, "@media x { a { color: red; } }", Token::Meta, "@media"));
        assert!(has(&JAVASCRIPT, "const s = `a\nb`;", Token::String, "b`"));
        assert!(has(&LUA, "--[[ x ]] local y = nil", Token::Comment, "--[[ x ]]"));
    }

    #[test]
    fn diffs_and_markdown_go_by_line() {
        let t = tokens(&DIFF, "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n same");
        assert_eq!(
            t.iter().map(|l| l.first().map(|(t, _)| *t)).collect::<Vec<_>>(),
            [Some(Token::Keyword), Some(Token::Keyword), Some(Token::Meta), Some(Token::Deleted), Some(Token::Inserted), None]
        );
        let t = tokens(&MARKDOWN, "# Title\nsome `code` here\n```\n# not a heading\n```\n> quote");
        assert_eq!(t[0], [(Token::Keyword, "# Title")]);
        assert_eq!(t[1], [(Token::String, "`code`")]);
        assert_eq!(t[3], [(Token::String, "# not a heading")]);
        assert_eq!(t[5], [(Token::Comment, "> quote")]);
    }

    #[test]
    fn long_lines_take_one_pass() {
        let line = "${a &b [c] key: 'd $x <e f=\"".repeat(20_000);
        let started = std::time::Instant::now();
        for lang in [&SHELL, &MARKUP, &YAML, &TOML, &RUST, &C, &MAKE] {
            assert_eq!(highlight(lang, &line).len(), 1);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn line_ends_and_odd_text_never_break_it() {
        let t = tokens(&RUST, "let a = 1;\r\nlet b = 2;\r\n");
        assert_eq!(t.len(), 3);
        assert!(t[0].iter().all(|(_, s)| !s.contains('\r')));
        // Every piece of text, with every language: no panics, ranges inside their lines.
        let odd = "é'\"\\`/*#@$ <a \"\n[\u{0}]--[[ 0x é1.e '\\";
        for lang in
            [&RUST, &PYTHON, &C, &CPP, &GO, &JAVASCRIPT, &TYPESCRIPT, &JAVA, &KOTLIN, &CSHARP, &SWIFT, &ZIG, &SHELL, &LUA, &RUBY, &SQL]
                .into_iter()
                .chain([&TOML, &INI, &YAML, &JSON, &MARKUP, &XML, &CSS, &MAKE, &DOCKERFILE, &DIFF, &MARKDOWN])
        {
            for end in 0..=odd.len() {
                let Some(text) = odd.get(..end) else { continue };
                let lines: Vec<&str> = text.split('\n').collect();
                for (spans, line) in highlight(lang, text).iter().zip(&lines) {
                    for (_, range) in spans {
                        assert!(line.get(range.clone()).is_some(), "{} {text:?}", lang.name);
                    }
                }
            }
        }
    }
}
