use crate::keywords::{lookup_keyword, Keyword};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    IntLiteral(i64),
    FloatLiteral(f32),
    DoubleLiteral(f64),
    StringLiteral(String),
    Identifier(String),
    Int,
    Float,
    Double,
    Bool,
    Str,
    Def,
    Class,
    Return,
    Public,
    Private,
    Default,
    Protect,
    Mut,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Import,
    Not,
    True,
    False,
    Colon,
    Comma,
    Dot,
    Equal,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Ampersand,
    Newline,
    Indent,
    Dedent,
    Plus,
    Minus,
    Star,
    Percent,
    Slash,
    LParen,
    RParen,
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        let mut indent_stack = vec![0usize];

        for raw_line in self.input.lines() {
            if raw_line.trim().is_empty() {
                continue;
            }

            if raw_line.starts_with('\t') {
                return Err("tabs are not supported for indentation".to_string());
            }

            let indent = raw_line.chars().take_while(|ch| *ch == ' ').count();
            let current_indent = *indent_stack.last().unwrap_or(&0);

            if indent > current_indent {
                indent_stack.push(indent);
                tokens.push(Token::Indent);
            } else if indent < current_indent {
                while indent < *indent_stack.last().unwrap_or(&0) {
                    indent_stack.pop();
                    tokens.push(Token::Dedent);
                }

                if indent != *indent_stack.last().unwrap_or(&0) {
                    return Err(format!("inconsistent indentation level: {indent}"));
                }
            }

            self.tokenize_line(&raw_line[indent..], &mut tokens)?;
            tokens.push(Token::Newline);
        }

        while indent_stack.len() > 1 {
            indent_stack.pop();
            tokens.push(Token::Dedent);
        }

        tokens.push(Token::Eof);
        Ok(tokens)
    }

    fn tokenize_line(&self, line: &str, tokens: &mut Vec<Token>) -> Result<(), String> {
        let mut chars = line.chars().peekable();

        while let Some(ch) = chars.peek().copied() {
            match ch {
                '0'..='9' => tokens.push(Self::read_number(&mut chars)?),
                '+' => {
                    chars.next();
                    tokens.push(Token::Plus);
                }
                '-' => {
                    chars.next();
                    tokens.push(Token::Minus);
                }
                '*' => {
                    chars.next();
                    tokens.push(Token::Star);
                }
                '%' => {
                    chars.next();
                    tokens.push(Token::Percent);
                }
                '/' => {
                    chars.next();
                    tokens.push(Token::Slash);
                }
                '"' => {
                    chars.next();
                    tokens.push(Self::read_string(&mut chars)?);
                }
                ':' => {
                    chars.next();
                    tokens.push(Token::Colon);
                }
                ',' => {
                    chars.next();
                    tokens.push(Token::Comma);
                }
                '.' => {
                    chars.next();
                    tokens.push(Token::Dot);
                }
                '&' => {
                    chars.next();
                    tokens.push(Token::Ampersand);
                }
                '=' => {
                    chars.next();
                    if matches!(chars.peek(), Some('=')) {
                        chars.next();
                        tokens.push(Token::EqualEqual);
                    } else {
                        tokens.push(Token::Equal);
                    }
                }
                '!' => {
                    chars.next();
                    if matches!(chars.peek(), Some('=')) {
                        chars.next();
                        tokens.push(Token::BangEqual);
                    } else {
                        return Err("unexpected character: !".to_string());
                    }
                }
                '<' => {
                    chars.next();
                    if matches!(chars.peek(), Some('=')) {
                        chars.next();
                        tokens.push(Token::LessEqual);
                    } else {
                        tokens.push(Token::Less);
                    }
                }
                '>' => {
                    chars.next();
                    if matches!(chars.peek(), Some('=')) {
                        chars.next();
                        tokens.push(Token::GreaterEqual);
                    } else {
                        tokens.push(Token::Greater);
                    }
                }
                '(' => {
                    chars.next();
                    tokens.push(Token::LParen);
                }
                ')' => {
                    chars.next();
                    tokens.push(Token::RParen);
                }
                c if c.is_ascii_alphabetic() || c == '_' => {
                    let ident = Self::read_identifier(&mut chars);
                    tokens.push(Self::keyword_or_identifier(ident));
                }
                c if c.is_whitespace() => {
                    chars.next();
                }
                _ => return Err(format!("unexpected character: {ch}")),
            }
        }

        Ok(())
    }

    fn read_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Token, String> {
        let mut buffer = String::new();
        let mut seen_dot = false;

        while let Some(ch) = chars.peek().copied() {
            match ch {
                '0'..='9' => {
                    buffer.push(ch);
                    chars.next();
                }
                '.' if !seen_dot => {
                    seen_dot = true;
                    buffer.push(ch);
                    chars.next();
                }
                _ => break,
            }
        }

        // Check for suffix (f for float, d for double)
        if let Some('f' | 'd' | 'F' | 'D') = chars.peek().copied() {
            let suffix = chars.next().unwrap();
            match suffix {
                'f' | 'F' => {
                    // Float literal (f32)
                    if !seen_dot {
                        // 1f는 1.0f로 취급
                        buffer.push('.');
                        buffer.push('0');
                    }
                    buffer
                        .parse::<f32>()
                        .map(Token::FloatLiteral)
                        .map_err(|_| format!("invalid float literal: {buffer}"))
                }
                'd' | 'D' => {
                    // Double literal (f64) - 명시적 지정
                    if !seen_dot {
                        buffer.push('.');
                        buffer.push('0');
                    }
                    buffer
                        .parse::<f64>()
                        .map(Token::DoubleLiteral)
                        .map_err(|_| format!("invalid double literal: {buffer}"))
                }
                _ => unreachable!(),
            }
        } else if seen_dot {
            // 접미사 없는 부동소수점 → Double (f64)
            buffer
                .parse::<f64>()
                .map(Token::DoubleLiteral)
                .map_err(|_| format!("invalid double literal: {buffer}"))
        } else {
            // 정수 리터럴
            buffer
                .parse::<i64>()
                .map(Token::IntLiteral)
                .map_err(|_| format!("invalid integer literal: {buffer}"))
        }
    }

    fn read_identifier(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
        let mut buffer = String::new();

        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                buffer.push(ch);
                chars.next();
            } else {
                break;
            }
        }

        buffer
    }

    fn read_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Token, String> {
        let mut buffer = String::new();

        while let Some(ch) = chars.next() {
            match ch {
                '"' => return Ok(Token::StringLiteral(buffer)),
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| "unterminated escape sequence".to_string())?;
                    match escaped {
                        '"' => buffer.push('"'),
                        '\\' => buffer.push('\\'),
                        'n' => buffer.push('\n'),
                        't' => buffer.push('\t'),
                        other => return Err(format!("unsupported escape: \\{other}")),
                    }
                }
                other => buffer.push(other),
            }
        }

        Err("unterminated string literal".to_string())
    }

    fn keyword_or_identifier(ident: String) -> Token {
        match lookup_keyword(&ident) {
            Some(Keyword::Int) => Token::Int,
            Some(Keyword::Float) => Token::Float,
            Some(Keyword::Double) => Token::Double,
            Some(Keyword::Bool) => Token::Bool,
            Some(Keyword::Str) => Token::Str,
            Some(Keyword::Def) => Token::Def,
            Some(Keyword::Class) => Token::Class,
            Some(Keyword::Return) => Token::Return,
            Some(Keyword::Public) => Token::Public,
            Some(Keyword::Private) => Token::Private,
            Some(Keyword::Default) => Token::Default,
            Some(Keyword::Protect) => Token::Protect,
            Some(Keyword::Mut) => Token::Mut,
            Some(Keyword::If) => Token::If,
            Some(Keyword::Elif) => Token::Elif,
            Some(Keyword::Else) => Token::Else,
            Some(Keyword::While) => Token::While,
            Some(Keyword::For) => Token::For,
            Some(Keyword::In) => Token::In,
            Some(Keyword::Import) => Token::Import,
            Some(Keyword::Not) => Token::Not,
            Some(Keyword::True) => Token::True,
            Some(Keyword::False) => Token::False,
            None => Token::Identifier(ident),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lexer, Token};

    #[test]
    fn tokenizes_arithmetic_expression() {
        let mut lexer = Lexer::new("12 + 3.5*(4-1)");
        let tokens = lexer.tokenize().unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::IntLiteral(12),
                Token::Plus,
                Token::DoubleLiteral(3.5),
                Token::Star,
                Token::LParen,
                Token::IntLiteral(4),
                Token::Minus,
                Token::IntLiteral(1),
                Token::RParen,
                Token::Newline,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_indented_if_block() {
        let mut lexer = Lexer::new("if score == 30:\n    print(score)\nelse:\n    print(20)\n");
        let tokens = lexer.tokenize().unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::If,
                Token::Identifier("score".to_string()),
                Token::EqualEqual,
                Token::IntLiteral(30),
                Token::Colon,
                Token::Newline,
                Token::Indent,
                Token::Identifier("print".to_string()),
                Token::LParen,
                Token::Identifier("score".to_string()),
                Token::RParen,
                Token::Newline,
                Token::Dedent,
                Token::Else,
                Token::Colon,
                Token::Newline,
                Token::Indent,
                Token::Identifier("print".to_string()),
                Token::LParen,
                Token::IntLiteral(20),
                Token::RParen,
                Token::Newline,
                Token::Dedent,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_typed_and_borrowed_declarations() {
        let mut lexer = Lexer::new("int score = 42\n&int shared = score\n&mut int pinned = score\nbool ready = True\nstr name = \"yuumi\"\n");
        let tokens = lexer.tokenize().unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Int,
                Token::Identifier("score".to_string()),
                Token::Equal,
                Token::IntLiteral(42),
                Token::Newline,
                Token::Ampersand,
                Token::Int,
                Token::Identifier("shared".to_string()),
                Token::Equal,
                Token::Identifier("score".to_string()),
                Token::Newline,
                Token::Ampersand,
                Token::Mut,
                Token::Int,
                Token::Identifier("pinned".to_string()),
                Token::Equal,
                Token::Identifier("score".to_string()),
                Token::Newline,
                Token::Bool,
                Token::Identifier("ready".to_string()),
                Token::Equal,
                Token::True,
                Token::Newline,
                Token::Str,
                Token::Identifier("name".to_string()),
                Token::Equal,
                Token::StringLiteral("yuumi".to_string()),
                Token::Newline,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_loop_keywords() {
        let mut lexer = Lexer::new("for i in range(3):\n    i\nwhile False:\n    0\n");
        let tokens = lexer.tokenize().unwrap();

        assert!(tokens.contains(&Token::For));
        assert!(tokens.contains(&Token::In));
        assert!(tokens.contains(&Token::While));
    }
}

