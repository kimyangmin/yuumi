#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Int,
    Float,
    Double,
    Bool,
    Str,
    Mut,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Not,
    True,
    False,
}

pub fn lookup_keyword(ident: &str) -> Option<Keyword> {
    match ident {
        "int" => Some(Keyword::Int),
        "float" => Some(Keyword::Float),
        "double" => Some(Keyword::Double),
        "bool" => Some(Keyword::Bool),
        "str" => Some(Keyword::Str),
        "mut" => Some(Keyword::Mut),
        "if" => Some(Keyword::If),
        "elif" => Some(Keyword::Elif),
        "else" => Some(Keyword::Else),
        "while" => Some(Keyword::While),
        "for" => Some(Keyword::For),
        "in" => Some(Keyword::In),
        "not" => Some(Keyword::Not),
        "True" | "true" => Some(Keyword::True),
        "False" | "false" => Some(Keyword::False),
        _ => None,
    }
}

