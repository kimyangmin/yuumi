use crate::builtins::is_range_function;
use crate::lexer::Token;
use crate::runtime::{BindingMode, TypeName};

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    VarDecl {
        binding: BindingMode,
        name: String,
        ty: TypeName,
        value: Expr,
    },
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    ForRange {
        name: String,
        start: Expr,
        end: Expr,
        body: Vec<Stmt>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    IntLiteral(i64),
    FloatLiteral(f32),
    DoubleLiteral(f64),
    StringLiteral(String),
    BoolLiteral(bool),
    Variable(String),
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(&mut self) -> Result<Program, String> {
        self.consume_newlines();

        let mut statements = Vec::new();
        while !matches!(self.peek(), Token::Eof) {
            statements.push(self.parse_statement()?);
            self.consume_newlines();
        }

        Ok(Program { statements })
    }

    fn parse_statement(&mut self) -> Result<Stmt, String> {
        match self.peek() {
            Token::If => self.parse_if_statement(),
            Token::While => self.parse_while_statement(),
            Token::For => self.parse_for_statement(),
            token if Self::is_decl_start(token) => self.parse_var_decl(),
            _ => Ok(Stmt::Expr(self.parse_expression()?)),
        }
    }

    fn is_decl_start(token: &Token) -> bool {
        matches!(token, Token::Int | Token::Float | Token::Double | Token::Bool | Token::Str | Token::Ampersand)
    }

    fn parse_var_decl(&mut self) -> Result<Stmt, String> {
        let binding = if matches!(self.peek(), Token::Ampersand) {
            self.advance();
            if matches!(self.peek(), Token::Mut) {
                self.advance();
                BindingMode::MutableBorrow
            } else {
                BindingMode::SharedBorrow
            }
        } else {
            BindingMode::Owned
        };

        let ty = self.parse_type_name()?;
        let name = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected variable name, found {other:?}")),
        };

        self.expect(Token::Equal, "expected '=' in declaration")?;
        let value = self.parse_expression()?;

        Ok(Stmt::VarDecl {
            binding,
            name,
            ty,
            value,
        })
    }

    fn parse_type_name(&mut self) -> Result<TypeName, String> {
        match self.advance() {
            Token::Int => Ok(TypeName::Int),
            Token::Float => Ok(TypeName::Float),
            Token::Double => Ok(TypeName::Double),
            Token::Bool => Ok(TypeName::Bool),
            Token::Str => Ok(TypeName::Str),
            other => Err(format!("expected type name, found {other:?}")),
        }
    }

    fn parse_if_statement(&mut self) -> Result<Stmt, String> {
        let mut branches = Vec::new();

        self.expect(Token::If, "expected 'if'")?;
        let condition = self.parse_expression()?;
        self.expect(Token::Colon, "expected ':' after if condition")?;
        branches.push((condition, self.parse_suite()?));

        while matches!(self.peek(), Token::Elif) {
            self.advance();
            let condition = self.parse_expression()?;
            self.expect(Token::Colon, "expected ':' after elif condition")?;
            branches.push((condition, self.parse_suite()?));
        }

        self.expect(Token::Else, "expected 'else' after if/elif chain")?;
        self.expect(Token::Colon, "expected ':' after else")?;
        let else_branch = self.parse_suite()?;

        Ok(Stmt::If {
            branches,
            else_branch,
        })
    }

    fn parse_while_statement(&mut self) -> Result<Stmt, String> {
        self.expect(Token::While, "expected 'while'")?;
        let condition = self.parse_expression()?;
        self.expect(Token::Colon, "expected ':' after while condition")?;
        let body = self.parse_suite()?;
        Ok(Stmt::While { condition, body })
    }

    fn parse_for_statement(&mut self) -> Result<Stmt, String> {
        self.expect(Token::For, "expected 'for'")?;
        let name = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected loop variable after 'for', found {other:?}")),
        };
        self.expect(Token::In, "expected 'in' after loop variable")?;

        let callee = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected range call after 'in', found {other:?}")),
        };
        if !is_range_function(&callee) {
            return Err(format!("for-loop currently supports only range(...), found '{callee}'"));
        }

        self.expect(Token::LParen, "expected '(' after range")?;
        let (start, end) = self.parse_range_args()?;
        self.expect(Token::RParen, "expected ')' after range arguments")?;
        self.expect(Token::Colon, "expected ':' after for header")?;
        let body = self.parse_suite()?;

        Ok(Stmt::ForRange {
            name,
            start,
            end,
            body,
        })
    }

    fn parse_range_args(&mut self) -> Result<(Expr, Expr), String> {
        if matches!(self.peek(), Token::RParen) {
            return Err("range() requires at least one argument".to_string());
        }

        let first = self.parse_expression()?;
        if matches!(self.peek(), Token::Comma) {
            self.advance();
            let second = self.parse_expression()?;
            if matches!(self.peek(), Token::Comma) {
                return Err("range() supports up to two arguments".to_string());
            }
            Ok((first, second))
        } else {
            Ok((Expr::IntLiteral(0), first))
        }
    }

    fn parse_suite(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(Token::Newline, "expected newline after ':'")?;
        self.expect(Token::Indent, "expected indented block")?;

        let mut statements = Vec::new();
        self.consume_newlines();
        while !matches!(self.peek(), Token::Dedent | Token::Eof) {
            statements.push(self.parse_statement()?);
            self.consume_newlines();
        }

        self.expect(Token::Dedent, "expected block end")?;
        Ok(statements)
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;

        loop {
            let op = match self.peek() {
                Token::EqualEqual => BinaryOp::Eq,
                Token::BangEqual => BinaryOp::NotEq,
                Token::Less => BinaryOp::Lt,
                Token::LessEqual => BinaryOp::Lte,
                Token::Greater => BinaryOp::Gt,
                Token::GreaterEqual => BinaryOp::Gte,
                _ => break,
            };
            self.advance();

            let right = self.parse_additive()?;
            left = Expr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_term()?;

        loop {
            let op = match self.peek() {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();

            let right = self.parse_term()?;
            left = Expr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;

        loop {
            let op = match self.peek() {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                _ => break,
            };
            self.advance();

            let right = self.parse_unary()?;
            left = Expr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Token::Minus => {
                self.advance();
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            Token::Not => {
                self.advance();
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.advance() {
            Token::IntLiteral(value) => Ok(Expr::IntLiteral(value)),
            Token::FloatLiteral(value) => Ok(Expr::FloatLiteral(value)),
            Token::DoubleLiteral(value) => Ok(Expr::DoubleLiteral(value)),
            Token::StringLiteral(value) => Ok(Expr::StringLiteral(value)),
            Token::True => Ok(Expr::BoolLiteral(true)),
            Token::False => Ok(Expr::BoolLiteral(false)),
            Token::Identifier(name) => {
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let mut args = Vec::new();

                    if !matches!(self.peek(), Token::RParen) {
                        loop {
                            args.push(self.parse_expression()?);
                            if matches!(self.peek(), Token::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }

                    self.expect(Token::RParen, "expected ')' after call arguments")?;
                    Ok(Expr::Call { name, args })
                } else {
                    Ok(Expr::Variable(name))
                }
            }
            Token::LParen => {
                let expr = self.parse_expression()?;
                self.expect(Token::RParen, "expected ')' after expression")?;
                Ok(expr)
            }
            other => Err(format!("unexpected token in expression: {other:?}")),
        }
    }

    fn consume_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    fn expect(&mut self, expected: Token, message: &str) -> Result<(), String> {
        let found = self.advance();
        if found == expected {
            Ok(())
        } else {
            Err(format!("{message}, found {found:?}"))
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos = self.pos.saturating_add(1);
        token
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryOp, Expr, Parser, Stmt, UnaryOp};
    use crate::lexer::{Lexer, Token};
    use crate::runtime::{BindingMode, TypeName};

    #[test]
    fn parses_precedence_correctly() {
        let mut lexer = Lexer::new("1 + 2 * 3\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements,
            vec![Stmt::Expr(Expr::Binary {
                left: Box::new(Expr::IntLiteral(1)),
                op: BinaryOp::Add,
                right: Box::new(Expr::Binary {
                    left: Box::new(Expr::IntLiteral(2)),
                    op: BinaryOp::Mul,
                    right: Box::new(Expr::IntLiteral(3)),
                }),
            })]
        );
    }

    #[test]
    fn parses_if_elif_else_statement() {
        let mut lexer = Lexer::new("if score == 30:\n    print(score)\nelif False:\n    print(2)\nelse:\n    print(3)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(program.statements.len(), 1);
        assert!(matches!(program.statements[0], Stmt::If { .. }));
    }

    #[test]
    fn parses_typed_variable_declaration() {
        let mut lexer = Lexer::new("double ratio = 1.5\nstr name = \"yuumi\"\nprint(name)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements,
            vec![
                Stmt::VarDecl {
                    binding: BindingMode::Owned,
                    name: "ratio".to_string(),
                    ty: TypeName::Double,
                    value: Expr::DoubleLiteral(1.5),
                },
                Stmt::VarDecl {
                    binding: BindingMode::Owned,
                    name: "name".to_string(),
                    ty: TypeName::Str,
                    value: Expr::StringLiteral("yuumi".to_string()),
                },
                Stmt::Expr(Expr::Call {
                    name: "print".to_string(),
                    args: vec![Expr::Variable("name".to_string())],
                }),
            ]
        );
    }

    #[test]
    fn parses_borrow_declaration() {
        let mut lexer = Lexer::new("int score = 30\n&int view = score\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements[1],
            Stmt::VarDecl {
                binding: BindingMode::SharedBorrow,
                name: "view".to_string(),
                ty: TypeName::Int,
                value: Expr::Variable("score".to_string()),
            }
        );
    }

    #[test]
    fn parses_not_expression() {
        let mut lexer = Lexer::new("not False\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements,
            vec![Stmt::Expr(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(Expr::BoolLiteral(false)),
            })]
        );
    }

    #[test]
    fn parses_while_statement() {
        let mut lexer = Lexer::new("while False:\n    1\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert!(matches!(program.statements[0], Stmt::While { .. }));
    }

    #[test]
    fn parses_for_range_statement() {
        let mut lexer = Lexer::new("for i in range(1, 3):\n    print(i)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert!(matches!(program.statements[0], Stmt::ForRange { .. }));
    }

    #[test]
    fn fails_when_token_stream_is_invalid() {
        let mut parser = Parser::new(vec![Token::Plus, Token::Newline, Token::Eof]);
        let err = parser.parse().unwrap_err();
        assert!(err.contains("unexpected token"));
    }
}
