use std::collections::HashMap;

use crate::builtins::BuiltinFunction;
use crate::parser::{BinaryOp, Expr, Program, Stmt, UnaryOp};
use crate::runtime::{convert_value, BindingMode, BorrowState, Reference, TypeName, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledProgram {
    pub instructions: Vec<Instruction>,
    pub global_types: Vec<TypeName>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    PushConst(Value),
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Not,
    CmpEq,
    CmpNotEq,
    CmpLt,
    CmpLte,
    CmpGt,
    CmpGte,
    JumpIfFalse(usize),
    Jump(usize),
    LoadGlobal(usize),
    StoreGlobal(usize),
    StoreBorrowGlobal { slot: usize, source: usize, mutable: bool },
    Print(usize),
    Println(usize),
    Pop,
    Halt,
}

pub struct BytecodeCompiler;

impl BytecodeCompiler {
    pub fn new() -> Self {
        Self
    }

    pub fn compile_program(&self, parsed: &Program) -> Result<CompiledProgram, String> {
        let mut instructions = Vec::new();
        let mut symbols = SymbolTable::default();

        for (idx, stmt) in parsed.statements.iter().enumerate() {
            self.compile_stmt(stmt, &mut instructions, &mut symbols)?;
            if idx + 1 != parsed.statements.len() {
                instructions.push(Instruction::Pop);
            }
        }

        instructions.push(Instruction::Halt);
        Ok(CompiledProgram {
            instructions,
            global_types: symbols.types,
        })
    }

    fn compile_stmt(
        &self,
        stmt: &Stmt,
        instructions: &mut Vec<Instruction>,
        symbols: &mut SymbolTable,
    ) -> Result<(), String> {
        match stmt {
            Stmt::Expr(expr) => self.compile_expr(expr, instructions, symbols),
            Stmt::VarDecl {
                binding,
                name,
                ty,
                value,
            } => {
                match binding {
                    BindingMode::Owned => {
                        self.compile_expr(value, instructions, symbols)?;
                        let slot = symbols.declare(name, *ty)?;
                        instructions.push(Instruction::StoreGlobal(slot));
                    }
                    BindingMode::SharedBorrow | BindingMode::MutableBorrow => {
                        let Expr::Variable(source_name) = value else {
                            return Err("borrow declarations must reference a variable".to_string());
                        };
                        let source = symbols
                            .resolve(source_name)
                            .ok_or_else(|| format!("undefined variable: {source_name}"))?;
                        let source_ty = symbols.types[source];
                        if source_ty != *ty {
                            return Err(format!(
                                "borrow type mismatch: expected '{}', found '{}'",
                                ty.keyword(),
                                source_ty.keyword()
                            ));
                        }
                        let slot = symbols.declare(name, *ty)?;
                        instructions.push(Instruction::StoreBorrowGlobal {
                            slot,
                            source,
                            mutable: matches!(binding, BindingMode::MutableBorrow),
                        });
                    }
                }
                instructions.push(Instruction::PushConst(Value::Unit));
                Ok(())
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                let mut end_jumps = Vec::new();
                for (condition, body) in branches {
                    self.compile_expr(condition, instructions, symbols)?;
                    let jump_if_false_pos = instructions.len();
                    instructions.push(Instruction::JumpIfFalse(usize::MAX));
                    self.compile_block(body, instructions, symbols)?;
                    let jump_pos = instructions.len();
                    instructions.push(Instruction::Jump(usize::MAX));
                    end_jumps.push(jump_pos);
                    let next_ip = instructions.len();
                    instructions[jump_if_false_pos] = Instruction::JumpIfFalse(next_ip);
                }
                self.compile_block(else_branch, instructions, symbols)?;
                let end_ip = instructions.len();
                for pos in end_jumps {
                    instructions[pos] = Instruction::Jump(end_ip);
                }
                Ok(())
            }
            Stmt::While { condition, body } => {
                let loop_start = instructions.len();
                self.compile_expr(condition, instructions, symbols)?;
                let jump_if_false_pos = instructions.len();
                instructions.push(Instruction::JumpIfFalse(usize::MAX));

                self.compile_block(body, instructions, symbols)?;
                instructions.push(Instruction::Pop);
                instructions.push(Instruction::Jump(loop_start));

                let loop_end = instructions.len();
                instructions[jump_if_false_pos] = Instruction::JumpIfFalse(loop_end);
                instructions.push(Instruction::PushConst(Value::Unit));
                Ok(())
            }
            Stmt::ForRange {
                name,
                start,
                end,
                body,
            } => {
                let iter_slot = match symbols.resolve(name) {
                    Some(slot) => slot,
                    None => symbols.declare(name, TypeName::Int)?,
                };
                let end_slot_name = format!("__for_end_{}", symbols.types.len());
                let end_slot = symbols.declare(&end_slot_name, TypeName::Int)?;

                self.compile_expr(start, instructions, symbols)?;
                instructions.push(Instruction::StoreGlobal(iter_slot));
                self.compile_expr(end, instructions, symbols)?;
                instructions.push(Instruction::StoreGlobal(end_slot));

                let loop_start = instructions.len();
                instructions.push(Instruction::LoadGlobal(iter_slot));
                instructions.push(Instruction::LoadGlobal(end_slot));
                instructions.push(Instruction::CmpLt);
                let jump_if_false_pos = instructions.len();
                instructions.push(Instruction::JumpIfFalse(usize::MAX));

                self.compile_block(body, instructions, symbols)?;
                instructions.push(Instruction::Pop);

                instructions.push(Instruction::LoadGlobal(iter_slot));
                instructions.push(Instruction::PushConst(Value::Int(1)));
                instructions.push(Instruction::Add);
                instructions.push(Instruction::StoreGlobal(iter_slot));
                instructions.push(Instruction::Jump(loop_start));

                let loop_end = instructions.len();
                instructions[jump_if_false_pos] = Instruction::JumpIfFalse(loop_end);
                instructions.push(Instruction::PushConst(Value::Unit));
                Ok(())
            }
        }
    }

    fn compile_block(
        &self,
        block: &[Stmt],
        instructions: &mut Vec<Instruction>,
        symbols: &mut SymbolTable,
    ) -> Result<(), String> {
        for (idx, stmt) in block.iter().enumerate() {
            self.compile_stmt(stmt, instructions, symbols)?;
            if idx + 1 != block.len() {
                instructions.push(Instruction::Pop);
            }
        }
        Ok(())
    }

    fn compile_expr(
        &self,
        expr: &Expr,
        instructions: &mut Vec<Instruction>,
        symbols: &SymbolTable,
    ) -> Result<(), String> {
        match expr {
            Expr::IntLiteral(value) => instructions.push(Instruction::PushConst(Value::Int(*value))),
            Expr::FloatLiteral(value) => instructions.push(Instruction::PushConst(Value::Float(*value))),
            Expr::DoubleLiteral(value) => instructions.push(Instruction::PushConst(Value::Double(*value))),
            Expr::StringLiteral(value) => instructions.push(Instruction::PushConst(Value::Str(value.clone()))),
            Expr::BoolLiteral(value) => instructions.push(Instruction::PushConst(Value::Bool(*value))),
            Expr::Variable(name) => {
                let slot = symbols
                    .resolve(name)
                    .ok_or_else(|| format!("undefined variable: {name}"))?;
                instructions.push(Instruction::LoadGlobal(slot));
            }
            Expr::Call { name, args } => {
                match BuiltinFunction::from_name(name) {
                    Some(BuiltinFunction::Print) => {
                        for arg in args {
                            self.compile_expr(arg, instructions, symbols)?;
                        }
                        instructions.push(Instruction::Print(args.len()));
                    }
                    Some(BuiltinFunction::Println) => {
                        for arg in args {
                            self.compile_expr(arg, instructions, symbols)?;
                        }
                        instructions.push(Instruction::Println(args.len()));
                    }
                    _ => return Err(format!("unknown function: {name}")),
                }
            }
            Expr::Unary { op, expr } => {
                self.compile_expr(expr, instructions, symbols)?;
                instructions.push(match op {
                    UnaryOp::Neg => Instruction::Neg,
                    UnaryOp::Not => Instruction::Not,
                });
            }
            Expr::Binary { left, op, right } => {
                self.compile_expr(left, instructions, symbols)?;
                self.compile_expr(right, instructions, symbols)?;
                instructions.push(match op {
                    BinaryOp::Add => Instruction::Add,
                    BinaryOp::Sub => Instruction::Sub,
                    BinaryOp::Mul => Instruction::Mul,
                    BinaryOp::Div => Instruction::Div,
                    BinaryOp::Eq => Instruction::CmpEq,
                    BinaryOp::NotEq => Instruction::CmpNotEq,
                    BinaryOp::Lt => Instruction::CmpLt,
                    BinaryOp::Lte => Instruction::CmpLte,
                    BinaryOp::Gt => Instruction::CmpGt,
                    BinaryOp::Gte => Instruction::CmpGte,
                });
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct SymbolTable {
    slots: HashMap<String, usize>,
    types: Vec<TypeName>,
}

impl SymbolTable {
    fn declare(&mut self, name: &str, ty: TypeName) -> Result<usize, String> {
        if self.slots.contains_key(name) {
            return Err(format!("variable '{name}' is already declared"));
        }
        let slot = self.types.len();
        self.slots.insert(name.to_string(), slot);
        self.types.push(ty);
        Ok(slot)
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        self.slots.get(name).copied()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct GlobalSlot {
    ty: TypeName,
    value: Value,
    borrow_state: BorrowState,
}

pub struct Vm;

impl Vm {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, program: &CompiledProgram) -> Result<Value, String> {
        let mut stack = Vec::new();
        let mut globals = program
            .global_types
            .iter()
            .copied()
            .map(|ty| GlobalSlot {
                ty,
                value: Value::Unit,
                borrow_state: BorrowState::default(),
            })
            .collect::<Vec<_>>();
        let mut ip = 0usize;

        while ip < program.instructions.len() {
            match &program.instructions[ip] {
                Instruction::PushConst(value) => stack.push(value.clone()),
                Instruction::Add => {
                    let (lhs, rhs) = pop_pair(&mut stack)?;
                    if let (Value::Str(lhs), Value::Str(rhs)) = (&lhs, &rhs) {
                        stack.push(Value::Str(format!("{lhs}{rhs}")));
                    } else {
                        stack.push(eval_arithmetic(lhs, rhs, |a, b| a + b, |a, b| a + b, |a, b| a + b)?);
                    }
                }
                Instruction::Sub => {
                    let (lhs, rhs) = pop_pair(&mut stack)?;
                    stack.push(eval_arithmetic(lhs, rhs, |a, b| a - b, |a, b| a - b, |a, b| a - b)?);
                }
                Instruction::Mul => {
                    let (lhs, rhs) = pop_pair(&mut stack)?;
                    stack.push(eval_arithmetic(lhs, rhs, |a, b| a * b, |a, b| a * b, |a, b| a * b)?);
                }
                Instruction::Div => {
                    let (lhs, rhs) = pop_pair(&mut stack)?;
                    stack.push(eval_div(lhs, rhs)?);
                }
                Instruction::Neg => {
                    let value = materialize(pop_value(&mut stack)?, &globals)?;
                    stack.push(match value {
                        Value::Int(value) => Value::Int(-value),
                        Value::Float(value) => Value::Float(-value),
                        Value::Double(value) => Value::Double(-value),
                        _ => return Err("unary '-' expects a numeric value".to_string()),
                    });
                }
                Instruction::Not => {
                    let value = materialize(pop_value(&mut stack)?, &globals)?;
                    stack.push(Value::Bool(!value.truthy()));
                }
                Instruction::CmpEq => compare_push(&mut stack, &globals, |ord| ord == 0)?,
                Instruction::CmpNotEq => compare_push(&mut stack, &globals, |ord| ord != 0)?,
                Instruction::CmpLt => compare_push(&mut stack, &globals, |ord| ord < 0)?,
                Instruction::CmpLte => compare_push(&mut stack, &globals, |ord| ord <= 0)?,
                Instruction::CmpGt => compare_push(&mut stack, &globals, |ord| ord > 0)?,
                Instruction::CmpGte => compare_push(&mut stack, &globals, |ord| ord >= 0)?,
                Instruction::JumpIfFalse(target) => {
                    let condition = materialize(pop_value(&mut stack)?, &globals)?;
                    if !condition.truthy() {
                        ip = *target;
                        continue;
                    }
                }
                Instruction::Jump(target) => {
                    ip = *target;
                    continue;
                }
                Instruction::LoadGlobal(slot) => {
                    let value = globals
                        .get(*slot)
                        .map(|entry| entry.value.clone())
                        .ok_or_else(|| format!("invalid global slot: {slot}"))?;
                    stack.push(materialize(value, &globals)?);
                }
                Instruction::StoreGlobal(slot) => {
                    let value = materialize(pop_value(&mut stack)?, &globals)?;
                    let entry = globals
                        .get_mut(*slot)
                        .ok_or_else(|| format!("invalid global slot: {slot}"))?;
                    entry.value = convert_value(value, entry.ty)?;
                }
                Instruction::StoreBorrowGlobal {
                    slot,
                    source,
                    mutable,
                } => {
                    if *slot == *source {
                        return Err("internal error: borrow slot/source collision".to_string());
                    }
                    let source_entry = globals
                        .get_mut(*source)
                        .ok_or_else(|| format!("invalid global slot: {source}"))?;
                    if *mutable {
                        if source_entry.borrow_state.mutable_active || source_entry.borrow_state.shared_count > 0 {
                            return Err("cannot mutably borrow while another borrow is active".to_string());
                        }
                        source_entry.borrow_state.mutable_active = true;
                    } else {
                        if source_entry.borrow_state.mutable_active {
                            return Err("cannot shared-borrow while mutable borrow is active".to_string());
                        }
                        source_entry.borrow_state.shared_count += 1;
                    }
                    let ty = source_entry.ty;
                    let entry = globals
                        .get_mut(*slot)
                        .ok_or_else(|| format!("invalid global slot: {slot}"))?;
                    entry.value = Value::Ref(Reference {
                        slot: *source,
                        mutable: *mutable,
                        ty,
                    });
                }
                Instruction::Print(arg_count) => {
                    if stack.len() < *arg_count {
                        return Err("stack underflow".to_string());
                    }
                    let start = stack.len() - *arg_count;
                    let values = stack.split_off(start);
                    let mut last_value = Value::Unit;
                    let mut rendered = Vec::with_capacity(values.len());
                    for value in values {
                        let materialized = materialize(value, &globals)?;
                        rendered.push(materialized.to_string());
                        last_value = materialized;
                    }
                    print!("{}", rendered.join(" "));
                    stack.push(last_value);
                }
                Instruction::Println(arg_count) => {
                    if stack.len() < *arg_count {
                        return Err("stack underflow".to_string());
                    }
                    let start = stack.len() - *arg_count;
                    let values = stack.split_off(start);
                    let mut last_value = Value::Unit;
                    let mut rendered = Vec::with_capacity(values.len());
                    for value in values {
                        let materialized = materialize(value, &globals)?;
                        rendered.push(materialized.to_string());
                        last_value = materialized;
                    }
                    println!("{}", rendered.join(" "));
                    stack.push(last_value);
                }
                Instruction::Pop => {
                    stack.pop().ok_or_else(|| "stack underflow".to_string())?;
                }
                Instruction::Halt => break,
            }
            ip += 1;
        }

        Ok(stack.pop().unwrap_or(Value::Unit))
    }
}

fn pop_value(stack: &mut Vec<Value>) -> Result<Value, String> {
    stack.pop().ok_or_else(|| "stack underflow".to_string())
}

fn pop_pair(stack: &mut Vec<Value>) -> Result<(Value, Value), String> {
    let rhs = pop_value(stack)?;
    let lhs = pop_value(stack)?;
    Ok((lhs, rhs))
}

fn materialize(value: Value, globals: &[GlobalSlot]) -> Result<Value, String> {
    match value {
        Value::Ref(reference) => {
            let value = globals
                .get(reference.slot)
                .map(|entry| entry.value.clone())
                .ok_or_else(|| format!("invalid global slot: {}", reference.slot))?;
            materialize(value, globals)
        }
        value => Ok(value),
    }
}

fn compare_push<F>(stack: &mut Vec<Value>, globals: &[GlobalSlot], predicate: F) -> Result<(), String>
where
    F: FnOnce(i8) -> bool,
{
    let (lhs, rhs) = pop_pair(stack)?;
    let lhs = materialize(lhs, globals)?;
    let rhs = materialize(rhs, globals)?;
    stack.push(Value::Bool(predicate(compare_values(lhs, rhs)?)));
    Ok(())
}

fn eval_arithmetic<FI, FF, FD>(
    lhs: Value,
    rhs: Value,
    int_op: FI,
    float_op: FF,
    double_op: FD,
) -> Result<Value, String>
where
    FI: FnOnce(i64, i64) -> i64,
    FF: FnOnce(f32, f32) -> f32,
    FD: FnOnce(f64, f64) -> f64,
{
    match common_numeric_type(&lhs, &rhs)? {
        TypeName::Int => Ok(Value::Int(int_op(as_int(&lhs)?, as_int(&rhs)?))),
        TypeName::Float => Ok(Value::Float(float_op(as_f32(&lhs)?, as_f32(&rhs)?))),
        TypeName::Double => Ok(Value::Double(double_op(as_f64(&lhs)?, as_f64(&rhs)?))),
        TypeName::Bool => Err("boolean values do not support arithmetic".to_string()),
        TypeName::Str => Err("string values do not support arithmetic".to_string()),
    }
}

fn eval_div(lhs: Value, rhs: Value) -> Result<Value, String> {
    match common_numeric_type(&lhs, &rhs)? {
        TypeName::Int => {
            let divisor = as_int(&rhs)?;
            if divisor == 0 {
                return Err("division by zero".to_string());
            }
            Ok(Value::Int(as_int(&lhs)? / divisor))
        }
        TypeName::Float => {
            let divisor = as_f32(&rhs)?;
            if divisor == 0.0 {
                return Err("division by zero".to_string());
            }
            Ok(Value::Float(as_f32(&lhs)? / divisor))
        }
        TypeName::Double => {
            let divisor = as_f64(&rhs)?;
            if divisor == 0.0 {
                return Err("division by zero".to_string());
            }
            Ok(Value::Double(as_f64(&lhs)? / divisor))
        }
        TypeName::Bool => Err("boolean values do not support division".to_string()),
        TypeName::Str => Err("string values do not support division".to_string()),
    }
}

fn common_numeric_type(lhs: &Value, rhs: &Value) -> Result<TypeName, String> {
    match (lhs.ty(), rhs.ty()) {
        (Some(TypeName::Double), Some(TypeName::Double | TypeName::Float | TypeName::Int))
        | (Some(TypeName::Float | TypeName::Int), Some(TypeName::Double)) => Ok(TypeName::Double),
        (Some(TypeName::Float), Some(TypeName::Float | TypeName::Int))
        | (Some(TypeName::Int), Some(TypeName::Float)) => Ok(TypeName::Float),
        (Some(TypeName::Int), Some(TypeName::Int)) => Ok(TypeName::Int),
        (Some(TypeName::Str), Some(TypeName::Str)) => Ok(TypeName::Str),
        _ => Err("numeric operator expects int/float/double operands".to_string()),
    }
}

fn compare_values(lhs: Value, rhs: Value) -> Result<i8, String> {
    match (&lhs, &rhs) {
        (Value::Bool(lhs), Value::Bool(rhs)) => Ok(if lhs == rhs { 0 } else if !*lhs && *rhs { -1 } else { 1 }),
        (Value::Str(lhs), Value::Str(rhs)) => Ok(if lhs < rhs { -1 } else if lhs > rhs { 1 } else { 0 }),
        _ => match common_numeric_type(&lhs, &rhs)? {
            TypeName::Int => Ok(if as_int(&lhs)? < as_int(&rhs)? {
                -1
            } else if as_int(&lhs)? > as_int(&rhs)? {
                1
            } else {
                0
            }),
            TypeName::Float => {
                let lhs = as_f32(&lhs)?;
                let rhs = as_f32(&rhs)?;
                Ok(if lhs < rhs { -1 } else if lhs > rhs { 1 } else { 0 })
            }
            TypeName::Double => {
                let lhs = as_f64(&lhs)?;
                let rhs = as_f64(&rhs)?;
                Ok(if lhs < rhs { -1 } else if lhs > rhs { 1 } else { 0 })
            }
            TypeName::Bool => Err("unreachable boolean compare".to_string()),
            TypeName::Str => Err("unreachable string compare".to_string()),
        },
    }
}

fn as_int(value: &Value) -> Result<i64, String> {
    match value {
        Value::Int(value) => Ok(*value),
        Value::Bool(value) => Ok(i64::from(*value)),
        _ => Err("expected int-compatible value".to_string()),
    }
}

fn as_f32(value: &Value) -> Result<f32, String> {
    match value {
        Value::Int(value) => Ok(*value as f32),
        Value::Float(value) => Ok(*value),
        Value::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
        _ => Err("expected float-compatible value".to_string()),
    }
}

fn as_f64(value: &Value) -> Result<f64, String> {
    match value {
        Value::Int(value) => Ok(*value as f64),
        Value::Float(value) => Ok(*value as f64),
        Value::Double(value) => Ok(*value),
        Value::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
        _ => Err("expected double-compatible value".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{BytecodeCompiler, Vm};
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::runtime::Value;

    #[test]
    fn compiles_and_runs_expression() {
        let mut lexer = Lexer::new("1 + 2 * (3 + 4) - 5\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Int(10));
    }

    #[test]
    fn runs_comparison_and_if() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    1\nelse:\n    0\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn runs_typed_numeric_program() {
        let mut lexer = Lexer::new("float a = 2\ndouble b = a / 4\nb\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Double(0.5));
    }

    #[test]
    fn supports_shared_borrow_reads() {
        let mut lexer = Lexer::new("int score = 30\n&int view = score\nview + 5\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Int(35));
    }

    #[test]
    fn supports_string_concat() {
        let mut lexer = Lexer::new("str a = \"yu\"\nstr b = \"umi\"\nprint(a + b)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Str("yuumi".to_string()));
    }

    #[test]
    fn rejects_conflicting_borrows() {
        let mut lexer = Lexer::new("int score = 30\n&mut int writer = score\n&int reader = score\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let err = Vm::new().run(&program).unwrap_err();
        assert!(err.contains("cannot shared-borrow"));
    }

    #[test]
    fn print_returns_last_argument() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    print(score)\nelse:\n    print(0)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn runs_for_range_loop() {
        let mut lexer = Lexer::new("for i in range(1, 4):\n    print(i)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();

        let program = BytecodeCompiler::new().compile_program(&ast).unwrap();
        let result = Vm::new().run(&program).unwrap();
        assert_eq!(result, Value::Unit);
    }
}

