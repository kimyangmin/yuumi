use std::collections::HashMap;

use crate::builtins::BuiltinFunction;
use crate::parser::{BinaryOp, Expr, Program, Stmt, UnaryOp};
use crate::runtime::{convert_value, BindingMode, BorrowState, Reference, TypeName, Value};

#[derive(Debug, Clone)]
struct VariableSlot {
    name: String,
    ty: TypeName,
    value: Value,
    borrow_state: BorrowState,
}

pub struct Interpreter {
    symbols: HashMap<String, usize>,
    slots: Vec<VariableSlot>,
    scopes: Vec<Vec<usize>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            slots: Vec::new(),
            scopes: vec![Vec::new()],
        }
    }

    pub fn execute_program(&mut self, program: &Program) -> Result<Value, String> {
        let mut last_value = Value::Unit;

        for stmt in &program.statements {
            last_value = self.execute_stmt(stmt)?;
        }

        Ok(last_value)
    }

    fn execute_stmt(&mut self, stmt: &Stmt) -> Result<Value, String> {
        match stmt {
            Stmt::Expr(expr) => self.eval_expr(expr),
            Stmt::VarDecl {
                binding,
                name,
                ty,
                value,
            } => {
                let stored = self.prepare_binding(*binding, *ty, value)?;
                self.declare(name.clone(), *ty, stored)?;
                Ok(Value::Unit)
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (condition, body) in branches {
                    if self.eval_expr(condition)?.truthy() {
                        return self.execute_block(body);
                    }
                }

                self.execute_block(else_branch)
            }
            Stmt::While { condition, body } => {
                let mut last_value = Value::Unit;
                while self.eval_expr(condition)?.truthy() {
                    last_value = self.execute_block(body)?;
                }
                Ok(last_value)
            }
            Stmt::ForRange {
                name,
                start,
                end,
                body,
            } => {
                let start_raw = self.eval_expr(start)?;
                let end_raw = self.eval_expr(end)?;
                let start_value = self.materialize_value(start_raw)?;
                let end_value = self.materialize_value(end_raw)?;
                let start = as_int(&start_value)?;
                let end = as_int(&end_value)?;

                let mut last_value = Value::Unit;
                for i in start..end {
                    self.scopes.push(Vec::new());
                    self.declare(name.clone(), TypeName::Int, Value::Int(i))?;
                    last_value = self.execute_block(body)?;
                    self.exit_scope()?;
                }
                Ok(last_value)
            }
        }
    }

    fn execute_block(&mut self, body: &[Stmt]) -> Result<Value, String> {
        self.scopes.push(Vec::new());
        let mut last = Value::Unit;

        for stmt in body {
            last = self.execute_stmt(stmt)?;
        }

        self.exit_scope()?;
        Ok(last)
    }

    fn prepare_binding(
        &mut self,
        binding: BindingMode,
        ty: TypeName,
        value_expr: &Expr,
    ) -> Result<Value, String> {
        match binding {
            BindingMode::Owned => {
                let value = self.eval_expr(value_expr)?;
                convert_value(self.materialize_value(value)?, ty)
            }
            BindingMode::SharedBorrow | BindingMode::MutableBorrow => {
                let Expr::Variable(target_name) = value_expr else {
                    return Err("borrow declarations must reference an existing variable".to_string());
                };
                let target_slot = self.resolve_slot(target_name)?;
                let target_ty = self.slots[target_slot].ty;
                if target_ty != ty {
                    return Err(format!(
                        "borrow type mismatch: expected '{}', found '{}'",
                        ty.keyword(),
                        target_ty.keyword()
                    ));
                }

                let state = &mut self.slots[target_slot].borrow_state;
                match binding {
                    BindingMode::SharedBorrow => {
                        if state.mutable_active {
                            return Err(format!(
                                "cannot shared-borrow '{target_name}' while mutable borrow is active"
                            ));
                        }
                        state.shared_count += 1;
                        Ok(Value::Ref(Reference {
                            slot: target_slot,
                            mutable: false,
                            ty,
                        }))
                    }
                    BindingMode::MutableBorrow => {
                        if state.mutable_active || state.shared_count > 0 {
                            return Err(format!(
                                "cannot mutably borrow '{target_name}' while another borrow is active"
                            ));
                        }
                        state.mutable_active = true;
                        Ok(Value::Ref(Reference {
                            slot: target_slot,
                            mutable: true,
                            ty,
                        }))
                    }
                    BindingMode::Owned => unreachable!(),
                }
            }
        }
    }

    fn declare(&mut self, name: String, ty: TypeName, value: Value) -> Result<(), String> {
        if self.symbols.contains_key(&name) {
            return Err(format!("variable '{name}' is already declared"));
        }

        let slot_index = self.slots.len();
        self.slots.push(VariableSlot {
            name: name.clone(),
            ty,
            value,
            borrow_state: BorrowState::default(),
        });
        self.symbols.insert(name, slot_index);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(slot_index);
        }
        Ok(())
    }

    fn exit_scope(&mut self) -> Result<(), String> {
        let scope = self
            .scopes
            .pop()
            .ok_or_else(|| "internal error: missing scope".to_string())?;

        for slot_index in scope.into_iter().rev() {
            let slot = self
                .slots
                .pop()
                .ok_or_else(|| "internal error: missing slot".to_string())?;
            if slot_index != self.slots.len() {
                return Err("internal error: non-LIFO scope pop".to_string());
            }
            if let Value::Ref(reference) = slot.value {
                let state = &mut self.slots[reference.slot].borrow_state;
                if reference.mutable {
                    state.mutable_active = false;
                } else if state.shared_count > 0 {
                    state.shared_count -= 1;
                }
            }
            self.symbols.remove(&slot.name);
        }

        if self.scopes.is_empty() {
            self.scopes.push(Vec::new());
        }
        Ok(())
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::IntLiteral(value) => Ok(Value::Int(*value)),
            Expr::FloatLiteral(value) => Ok(Value::Float(*value)),
            Expr::DoubleLiteral(value) => Ok(Value::Double(*value)),
            Expr::StringLiteral(value) => Ok(Value::Str(value.clone())),
            Expr::BoolLiteral(value) => Ok(Value::Bool(*value)),
            Expr::Variable(name) => {
                let slot = self.resolve_slot(name)?;
                self.read_slot(slot)
            }
            Expr::Call { name, args } => self.eval_call(name, args),
            Expr::Unary { op, expr } => {
                let raw = self.eval_expr(expr)?;
                let value = self.materialize_value(raw)?;
                self.eval_unary(*op, value)
            }
            Expr::Binary { left, op, right } => {
                let lhs_raw = self.eval_expr(left)?;
                let rhs_raw = self.eval_expr(right)?;
                let lhs = self.materialize_value(lhs_raw)?;
                let rhs = self.materialize_value(rhs_raw)?;
                self.eval_binary(lhs, *op, rhs)
            }
        }
    }

    fn eval_call(&mut self, name: &str, args: &[Expr]) -> Result<Value, String> {
        match BuiltinFunction::from_name(name) {
            Some(BuiltinFunction::Print) => {
                let mut rendered = Vec::with_capacity(args.len());
                let mut last_value = Value::Unit;
                for arg in args {
                    let raw = self.eval_expr(arg)?;
                    let value = self.materialize_value(raw)?;
                    rendered.push(value.to_string());
                    last_value = value;
                }
                print!("{}", rendered.join(" "));
                Ok(last_value)
            }
            Some(BuiltinFunction::Println) => {
                let mut rendered = Vec::with_capacity(args.len());
                let mut last_value = Value::Unit;
                for arg in args {
                    let raw = self.eval_expr(arg)?;
                    let value = self.materialize_value(raw)?;
                    rendered.push(value.to_string());
                    last_value = value;
                }
                println!("{}", rendered.join(" "));
                Ok(last_value)
            }
            _ => Err(format!("unknown function: {name}")),
        }
    }

    fn eval_unary(&self, op: UnaryOp, value: Value) -> Result<Value, String> {
        match op {
            UnaryOp::Neg => match value {
                Value::Int(value) => Ok(Value::Int(-value)),
                Value::Float(value) => Ok(Value::Float(-value)),
                Value::Double(value) => Ok(Value::Double(-value)),
                _ => Err("unary '-' expects a numeric value".to_string()),
            },
            UnaryOp::Not => Ok(Value::Bool(!value.truthy())),
        }
    }

    fn eval_binary(&self, lhs: Value, op: BinaryOp, rhs: Value) -> Result<Value, String> {
        match op {
            BinaryOp::Add => {
                if let (Value::Str(lhs), Value::Str(rhs)) = (&lhs, &rhs) {
                    return Ok(Value::Str(format!("{lhs}{rhs}")));
                }
                self.eval_arithmetic(lhs, rhs, |a, b| a + b, |a, b| a + b, |a, b| a + b)
            }
            BinaryOp::Sub => self.eval_arithmetic(lhs, rhs, |a, b| a - b, |a, b| a - b, |a, b| a - b),
            BinaryOp::Mul => {
                // String multiplication: "a" * 3 or 3 * "a"
                if let (Value::Str(s), Value::Int(n)) = (&lhs, &rhs) {
                    if *n < 0 {
                        return Err("string multiplication count cannot be negative".to_string());
                    }
                    return Ok(Value::Str(s.repeat(*n as usize)));
                }
                if let (Value::Int(n), Value::Str(s)) = (&lhs, &rhs) {
                    if *n < 0 {
                        return Err("string multiplication count cannot be negative".to_string());
                    }
                    return Ok(Value::Str(s.repeat(*n as usize)));
                }
                self.eval_arithmetic(lhs, rhs, |a, b| a * b, |a, b| a * b, |a, b| a * b)
            }
            BinaryOp::Div => self.eval_div(lhs, rhs),
            BinaryOp::Eq => self.eval_compare(lhs, rhs, |ord| ord == 0),
            BinaryOp::NotEq => self.eval_compare(lhs, rhs, |ord| ord != 0),
            BinaryOp::Lt => self.eval_compare(lhs, rhs, |ord| ord < 0),
            BinaryOp::Lte => self.eval_compare(lhs, rhs, |ord| ord <= 0),
            BinaryOp::Gt => self.eval_compare(lhs, rhs, |ord| ord > 0),
            BinaryOp::Gte => self.eval_compare(lhs, rhs, |ord| ord >= 0),
        }
    }

    fn eval_arithmetic<FI, FF, FD>(
        &self,
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

    fn eval_div(&self, lhs: Value, rhs: Value) -> Result<Value, String> {
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

    fn eval_compare<F>(&self, lhs: Value, rhs: Value, predicate: F) -> Result<Value, String>
    where
        F: FnOnce(i8) -> bool,
    {
        let ordering = compare_values(lhs, rhs)?;
        Ok(Value::Bool(predicate(ordering)))
    }

    fn resolve_slot(&self, name: &str) -> Result<usize, String> {
        self.symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("undefined variable: {name}"))
    }

    fn read_slot(&self, slot: usize) -> Result<Value, String> {
        let value = self
            .slots
            .get(slot)
            .map(|entry| entry.value.clone())
            .ok_or_else(|| format!("invalid variable slot: {slot}"))?;
        self.materialize_value(value)
    }

    fn materialize_value(&self, value: Value) -> Result<Value, String> {
        match value {
            Value::Ref(reference) => self.read_slot(reference.slot),
            value => Ok(value),
        }
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
            TypeName::Int => {
                let lhs = as_int(&lhs)?;
                let rhs = as_int(&rhs)?;
                Ok(if lhs < rhs { -1 } else if lhs > rhs { 1 } else { 0 })
            }
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
    use super::Interpreter;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::runtime::Value;

    #[test]
    fn evaluates_expression() {
        let mut lexer = Lexer::new("10 - 3 * (2 + 1)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn returns_error_on_division_by_zero() {
        let mut lexer = Lexer::new("8 / (2 - 2)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let err = interpreter.execute_program(&program).unwrap_err();
        assert_eq!(err, "division by zero");
    }

    #[test]
    fn evaluates_if_and_comparison() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    20\nelse:\n    10\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(20));
    }

    #[test]
    fn evaluates_typed_variables() {
        let mut lexer = Lexer::new("float speed = 7\ndouble ratio = speed / 2\nratio\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Double(3.5));
    }

    #[test]
    fn supports_shared_borrow_reads() {
        let mut lexer = Lexer::new("int score = 30\n&int view = score\nview + 2\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(32));
    }

    #[test]
    fn rejects_conflicting_borrows() {
        let mut lexer = Lexer::new("int score = 30\n&mut int writer = score\n&int reader = score\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let err = interpreter.execute_program(&program).unwrap_err();
        assert!(err.contains("cannot shared-borrow"));
    }

    #[test]
    fn print_returns_last_argument() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    print(score)\nelse:\n    print(0)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(30));
    }

    #[test]
    fn runs_for_range_and_while() {
        let mut lexer = Lexer::new(
            "for i in range(3):\n    print(i)\nwhile False:\n    1\n",
        );
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Unit);
    }

    #[test]
    fn supports_string_declaration_and_concat() {
        let mut lexer = Lexer::new("str a = \"yu\"\nstr b = \"umi\"\nprint(a + b)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let mut interpreter = Interpreter::new();
        let result = interpreter.execute_program(&program).unwrap();
        assert_eq!(result, Value::Str("yuumi".to_string()));
    }
}

