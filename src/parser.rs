use crate::builtins::is_range_function;
use crate::lexer::Token;
use crate::runtime::{BindingMode, TypeName};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    Public,
    Default,
    Private,
    Protect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeName>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Field {
        access: AccessLevel,
        name: String,
        ty: TypeName,
        value: Expr,
    },
    Method {
        access: AccessLevel,
        name: String,
        return_type: Option<TypeName>,
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Import {
        path: String,
    },
    FunctionDef {
        access: AccessLevel,
        return_type: Option<TypeName>,
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    ClassDef {
        name: String,
        base: Option<String>,
        members: Vec<ClassMember>,
    },
    VarDecl {
        binding: BindingMode,
        name: String,
        ty: TypeName,
        value: Expr,
    },
    Assign {
        name: String,
        value: Expr,
    },
    MemberAssign {
        object: Expr,
        member: String,
        value: Expr,
    },
    Swap {
        left: Vec<String>,
        right: Vec<String>,
    },
    Return(Option<Expr>),
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
    Member {
        object: Box<Expr>,
        member: String,
    },
    MethodCall {
        object: Box<Expr>,
        method: String,
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
    Mod,
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
            Token::Import => self.parse_import_statement(),
            Token::Public | Token::Private | Token::Default | Token::Protect => {
                let access = self.parse_access_level()?;
                if matches!(self.peek(), Token::Def) {
                    self.parse_function_def(access, None)
                } else if Self::is_type_name_start(self.peek()) && matches!(self.peek_offset(1), Token::Def) {
                    let return_type = self.parse_type_name()?;
                    self.parse_function_def(access, Some(return_type))
                } else {
                    Err("access modifier must be followed by def or '<type> def'".to_string())
                }
            }
            Token::Def => self.parse_function_def(AccessLevel::Default, None),
            token if Self::is_type_name_start(token) && matches!(self.peek_offset(1), Token::Def) => {
                let return_type = self.parse_type_name()?;
                self.parse_function_def(AccessLevel::Default, Some(return_type))
            }
            Token::Class => self.parse_class_def(),
            Token::Return => self.parse_return_statement(),
            Token::If => self.parse_if_statement(),
            Token::While => self.parse_while_statement(),
            Token::For => self.parse_for_statement(),
            Token::Identifier(_) if self.looks_like_named_decl() => self.parse_var_decl(),
            token if Self::is_decl_start(token) && !matches!(self.peek_offset(1), Token::LParen) => {
                self.parse_var_decl()
            }
            Token::Identifier(_) => {
                if let Some(stmt) = self.try_parse_assignment_like()? {
                    Ok(stmt)
                } else {
                    Ok(Stmt::Expr(self.parse_expression()?))
                }
            }
            _ => Ok(Stmt::Expr(self.parse_expression()?)),
        }
    }

    fn parse_import_statement(&mut self) -> Result<Stmt, String> {
        self.expect(Token::Import, "expected 'import'")?;
        let path = match self.advance() {
            Token::StringLiteral(path) => path,
            Token::Identifier(module) => format!("{module}.yu"),
            other => return Err(format!("expected module path after import, found {other:?}")),
        };
        Ok(Stmt::Import { path })
    }

    fn try_parse_assignment_like(&mut self) -> Result<Option<Stmt>, String> {
        let saved = self.pos;

        if matches!(self.peek_offset(1), Token::Comma) {
            let stmt = self.parse_swap_statement()?;
            return Ok(Some(stmt));
        }

        let target = self.parse_expression()?;
        if !matches!(self.peek(), Token::Equal) {
            self.pos = saved;
            return Ok(None);
        }
        self.advance();
        let value = self.parse_expression()?;

        match target {
            Expr::Variable(name) => Ok(Some(Stmt::Assign { name, value })),
            Expr::Member { object, member } => Ok(Some(Stmt::MemberAssign {
                object: *object,
                member,
                value,
            })),
            _ => Err("invalid assignment target".to_string()),
        }
    }

    fn parse_access_level(&mut self) -> Result<AccessLevel, String> {
        match self.advance() {
            Token::Public => Ok(AccessLevel::Public),
            Token::Private => Ok(AccessLevel::Private),
            Token::Default => Ok(AccessLevel::Default),
            Token::Protect => Ok(AccessLevel::Protect),
            other => Err(format!("expected access modifier, found {other:?}")),
        }
    }

    fn parse_return_statement(&mut self) -> Result<Stmt, String> {
        self.expect(Token::Return, "expected 'return'")?;
        if matches!(self.peek(), Token::Newline | Token::Dedent | Token::Eof) {
            Ok(Stmt::Return(None))
        } else {
            Ok(Stmt::Return(Some(self.parse_expression()?)))
        }
    }

    fn parse_function_def(&mut self, access: AccessLevel, return_type: Option<TypeName>) -> Result<Stmt, String> {
        self.expect(Token::Def, "expected 'def'")?;
        let name = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected function name, found {other:?}")),
        };
        self.expect(Token::LParen, "expected '(' after function name")?;
        let params = self.parse_params()?;
        self.expect(Token::RParen, "expected ')' after parameter list")?;
        self.expect(Token::Colon, "expected ':' after function signature")?;
        let body = self.parse_suite()?;
        Ok(Stmt::FunctionDef {
            access,
            return_type,
            name,
            params,
            body,
        })
    }

    fn parse_class_def(&mut self) -> Result<Stmt, String> {
        self.expect(Token::Class, "expected 'class'")?;
        let name = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected class name, found {other:?}")),
        };

        let base = if matches!(self.peek(), Token::LParen) {
            self.advance();
            let base = match self.advance() {
                Token::Identifier(base) => base,
                other => return Err(format!("expected base class name, found {other:?}")),
            };
            self.expect(Token::RParen, "expected ')' after base class")?;
            Some(base)
        } else {
            None
        };

        self.expect(Token::Colon, "expected ':' after class name")?;
        self.expect(Token::Newline, "expected newline after class header")?;
        self.expect(Token::Indent, "expected indented class body")?;

        let mut members = Vec::new();
        self.consume_newlines();
        while !matches!(self.peek(), Token::Dedent | Token::Eof) {
            members.push(self.parse_class_member()?);
            self.consume_newlines();
        }

        self.expect(Token::Dedent, "expected end of class block")?;
        Ok(Stmt::ClassDef { name, base, members })
    }

    fn parse_class_member(&mut self) -> Result<ClassMember, String> {
        let access = match self.peek() {
            Token::Public | Token::Private | Token::Default | Token::Protect => self.parse_access_level()?,
            _ => AccessLevel::Default,
        };

        let (return_type, expects_def) = if matches!(self.peek(), Token::Def) {
            (None, true)
        } else if Self::is_type_name_start(self.peek()) && matches!(self.peek_offset(1), Token::Def) {
            (Some(self.parse_type_name()?), true)
        } else {
            (None, false)
        };

        if expects_def {
            let stmt = self.parse_function_def(access, return_type)?;
                match stmt {
                    Stmt::FunctionDef { name, return_type, params, body, .. } => Ok(ClassMember::Method {
                        access,
                        name,
                        return_type,
                        params,
                        body,
                    }),
                    _ => unreachable!(),
                }
        } else {
            match self.peek() {
            Token::Identifier(_) if self.looks_like_named_decl() => {
                let decl = self.parse_var_decl()?;
                match decl {
                    Stmt::VarDecl { name, ty, value, .. } => Ok(ClassMember::Field {
                        access,
                        name,
                        ty,
                        value,
                    }),
                    _ => unreachable!(),
                }
            }
            token if Self::is_decl_start(token) && !matches!(self.peek_offset(1), Token::LParen) => {
                let decl = self.parse_var_decl()?;
                match decl {
                    Stmt::VarDecl { name, ty, value, .. } => Ok(ClassMember::Field {
                        access,
                        name,
                        ty,
                        value,
                    }),
                    _ => unreachable!(),
                }
            }
            _ => Err("class body supports only field declarations and def methods".to_string()),
            }
        }
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, String> {
        let mut params = Vec::new();
        if matches!(self.peek(), Token::RParen) {
            return Ok(params);
        }

        loop {
            let param = match (self.peek().clone(), self.peek_offset(1).clone()) {
                (Token::Identifier(name), Token::Comma | Token::RParen) => {
                    self.advance();
                    Param { name, ty: None }
                }
                _ => {
                    let ty = self.parse_type_name()?;
                    let name = match self.advance() {
                        Token::Identifier(name) => name,
                        other => return Err(format!("expected parameter name, found {other:?}")),
                    };
                    Param { name, ty: Some(ty) }
                }
            };
            params.push(param);

            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        Ok(params)
    }

    fn parse_swap_statement(&mut self) -> Result<Stmt, String> {
        let left = self.parse_identifier_tuple()?;
        if left.len() < 2 {
            return Err("swap requires at least two variables".to_string());
        }

        self.expect(Token::Equal, "expected '=' in swap statement")?;
        let right = self.parse_identifier_tuple()?;

        if left.len() != right.len() {
            return Err("swap requires the same number of variables on both sides".to_string());
        }

        let left_set: HashSet<&str> = left.iter().map(String::as_str).collect();
        let right_set: HashSet<&str> = right.iter().map(String::as_str).collect();

        if left_set.len() != left.len() || right_set.len() != right.len() {
            return Err("swap does not allow duplicate variable names".to_string());
        }

        if left_set != right_set {
            return Err("swap requires both sides to contain the same variable names".to_string());
        }

        Ok(Stmt::Swap { left, right })
    }

    fn parse_identifier_tuple(&mut self) -> Result<Vec<String>, String> {
        let mut names = Vec::new();

        let first = match self.advance() {
            Token::Identifier(name) => name,
            other => return Err(format!("expected variable name, found {other:?}")),
        };
        names.push(first);

        while matches!(self.peek(), Token::Comma) {
            self.advance();
            let name = match self.advance() {
                Token::Identifier(name) => name,
                other => return Err(format!("expected variable name after ',', found {other:?}")),
            };
            names.push(name);
        }

        Ok(names)
    }

    fn is_decl_start(token: &Token) -> bool {
        matches!(token, Token::Int | Token::Float | Token::Double | Token::Bool | Token::Str | Token::Ampersand)
    }

    fn is_type_name_start(token: &Token) -> bool {
        matches!(token, Token::Int | Token::Float | Token::Double | Token::Bool | Token::Str | Token::Identifier(_))
    }

    fn looks_like_named_decl(&self) -> bool {
        matches!(self.peek(), Token::Identifier(_)) && matches!(self.peek_offset(1), Token::Identifier(_))
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
            Token::Identifier(name) => Ok(TypeName::Named(name)),
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

        let else_branch = if matches!(self.peek(), Token::Else) {
            self.advance();
            self.expect(Token::Colon, "expected ':' after else")?;
            self.parse_suite()?
        } else {
            Vec::new()
        };

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
                Token::Percent => BinaryOp::Mod,
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
        let expr = match self.advance() {
            Token::IntLiteral(value) => Ok(Expr::IntLiteral(value)),
            Token::FloatLiteral(value) => Ok(Expr::FloatLiteral(value)),
            Token::DoubleLiteral(value) => Ok(Expr::DoubleLiteral(value)),
            Token::StringLiteral(value) => Ok(Expr::StringLiteral(value)),
            Token::True => Ok(Expr::BoolLiteral(true)),
            Token::False => Ok(Expr::BoolLiteral(false)),
            Token::Int => self.parse_keyword_call("int"),
            Token::Float => self.parse_keyword_call("float"),
            Token::Double => self.parse_keyword_call("double"),
            Token::Str => self.parse_keyword_call("str"),
            Token::Identifier(name) if name == "type" && matches!(self.peek(), Token::LParen) => {
                self.parse_named_call(name)
            }
            Token::Identifier(name) => {
                if matches!(self.peek(), Token::LParen) {
                    self.parse_named_call(name)
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
        }?;

        self.parse_postfix(expr)
    }

    fn parse_keyword_call(&mut self, name: &str) -> Result<Expr, String> {
        if !matches!(self.peek(), Token::LParen) {
            return Err(format!("unexpected type keyword '{name}' in expression"));
        }

        self.parse_named_call(name.to_string())
    }

    fn parse_named_call(&mut self, name: String) -> Result<Expr, String> {
        self.expect(Token::LParen, "expected '(' after function name")?;
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
    }

    fn parse_postfix(&mut self, mut expr: Expr) -> Result<Expr, String> {
        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    let member = match self.advance() {
                        Token::Identifier(name) => name,
                        other => return Err(format!("expected member name after '.', found {other:?}")),
                    };
                    if matches!(self.peek(), Token::LParen) {
                        self.expect(Token::LParen, "expected '(' after method name")?;
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
                        self.expect(Token::RParen, "expected ')' after method arguments")?;
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: member,
                            args,
                        };
                    } else {
                        expr = Expr::Member {
                            object: Box::new(expr),
                            member,
                        };
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
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

    fn peek_offset(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
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
    fn parses_swap_statement() {
        let mut lexer = Lexer::new("int a = 10\nint b = 20\na, b = b, a\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements[2],
            Stmt::Swap {
                left: vec!["a".to_string(), "b".to_string()],
                right: vec!["b".to_string(), "a".to_string()],
            }
        );
    }

    #[test]
    fn parses_multi_swap_statement() {
        let mut lexer = Lexer::new("int a = 10\nint b = 20\nint c = 30\na, b, c = c, b, a\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        assert_eq!(
            program.statements[3],
            Stmt::Swap {
                left: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                right: vec!["c".to_string(), "b".to_string(), "a".to_string()],
            }
        );
    }

    #[test]
    fn fails_when_token_stream_is_invalid() {
        let mut parser = Parser::new(vec![Token::Plus, Token::Newline, Token::Eof]);
        let err = parser.parse().unwrap_err();
        assert!(err.contains("unexpected token"));
    }
}
