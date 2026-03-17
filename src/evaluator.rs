use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::lexer::Lexer;
use crate::parser::{AccessLevel, BinaryOp, ClassMember, Expr, Param, Program, Stmt, UnaryOp};
use crate::parser::Parser;
use crate::runtime::{TypeName, Value};

#[derive(Clone)]
struct Variable {
    ty: TypeName,
    value: Value,
}

#[derive(Clone)]
struct FunctionDef {
    access: AccessLevel,
    name: String,
    _return_type: Option<TypeName>,
    params: Vec<Param>,
    body: Vec<Stmt>,
    owner_class: Option<String>,
}

#[derive(Clone)]
struct FieldDef {
    access: AccessLevel,
    name: String,
    ty: TypeName,
    value: Expr,
}

#[derive(Clone)]
struct ClassDefEval {
    name: String,
    base: Option<String>,
    fields: Vec<FieldDef>,
    methods: HashMap<String, FunctionDef>,
}

#[derive(Clone)]
struct ObjectInstance {
    fields: HashMap<String, Variable>,
}

enum Flow {
    Value(Value),
    Return(Value),
}

pub fn execute_program(program: &Program) -> Result<Value, String> {
    let mut evaluator = Evaluator::new();
    evaluator.execute_program(program)
}

struct Evaluator {
    globals: HashMap<String, Variable>,
    scopes: Vec<HashMap<String, Variable>>,
    functions: HashMap<String, FunctionDef>,
    classes: HashMap<String, ClassDefEval>,
    objects: Vec<ObjectInstance>,
    current_class: Vec<Option<String>>,
    imported_paths: HashSet<String>,
}

impl Evaluator {
    fn new() -> Self {
        Self {
            globals: HashMap::new(),
            scopes: Vec::new(),
            functions: HashMap::new(),
            classes: HashMap::new(),
            objects: Vec::new(),
            current_class: vec![None],
            imported_paths: HashSet::new(),
        }
    }

    fn execute_program(&mut self, program: &Program) -> Result<Value, String> {
        let mut last = Value::Unit;
        for stmt in &program.statements {
            match self.execute_stmt(stmt)? {
                Flow::Value(value) => last = value,
                Flow::Return(value) => return Ok(value),
            }
        }
        Ok(last)
    }

    fn execute_block(&mut self, body: &[Stmt]) -> Result<Flow, String> {
        self.scopes.push(HashMap::new());
        let mut last = Value::Unit;
        for stmt in body {
            match self.execute_stmt(stmt)? {
                Flow::Value(value) => last = value,
                Flow::Return(value) => {
                    self.scopes.pop();
                    return Ok(Flow::Return(value));
                }
            }
        }
        self.scopes.pop();
        Ok(Flow::Value(last))
    }

    fn execute_stmt(&mut self, stmt: &Stmt) -> Result<Flow, String> {
        match stmt {
            Stmt::Import { path } => {
                self.execute_import(path)?;
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Expr(expr) => Ok(Flow::Value(self.eval_expr(expr)?)),
            Stmt::FunctionDef { access, return_type, name, params, body } => {
                self.functions.insert(
                    name.clone(),
                    FunctionDef {
                        access: *access,
                        name: name.clone(),
                        _return_type: return_type.clone(),
                        params: params.clone(),
                        body: body.clone(),
                        owner_class: None,
                    },
                );
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::ClassDef { name, base, members } => {
                let mut fields = Vec::new();
                let mut methods = HashMap::new();
                for member in members {
                    match member {
                        ClassMember::Field { access, name: field_name, ty, value } => {
                            fields.push(FieldDef {
                                access: *access,
                                name: field_name.clone(),
                                ty: ty.clone(),
                                value: value.clone(),
                            });
                        }
                        ClassMember::Method { access, name: method_name, return_type, params, body } => {
                            methods.insert(
                                method_name.clone(),
                                FunctionDef {
                                    access: *access,
                                    name: method_name.clone(),
                                    _return_type: return_type.clone(),
                                    params: params.clone(),
                                    body: body.clone(),
                                    owner_class: Some(name.clone()),
                                },
                            );
                        }
                    }
                }
                self.classes.insert(
                    name.clone(),
                    ClassDefEval {
                        name: name.clone(),
                        base: base.clone(),
                        fields,
                        methods,
                    },
                );
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::VarDecl { name, ty, value, .. } => {
                let raw = self.eval_expr(value)?;
                let converted = self.convert_value(raw, ty)?;
                self.define_var(name.clone(), ty.clone(), converted);
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Assign { name, value } => {
                let current_ty = self.lookup_var(name)?.ty.clone();
                let raw = self.eval_expr(value)?;
                let converted = self.convert_value(raw, &current_ty)?;
                self.assign_var(name, converted)?;
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::MemberAssign { object, member, value } => {
                let obj = self.eval_expr(object)?;
                let raw = self.eval_expr(value)?;
                self.assign_member(obj, member, raw)?;
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Swap { left, right } => {
                let mut rhs_values = Vec::new();
                for rhs in right {
                    rhs_values.push(self.lookup_var(rhs)?.value.clone());
                }
                for (index, left_name) in left.iter().enumerate() {
                    let ty = self.lookup_var(left_name)?.ty.clone();
                    let converted = self.convert_value(rhs_values[index].clone(), &ty)?;
                    self.assign_var(left_name, converted)?;
                }
                Ok(Flow::Value(Value::Unit))
            }
            Stmt::Return(expr) => {
                let value = match expr {
                    Some(expr) => self.eval_expr(expr)?,
                    None => Value::Unit,
                };
                Ok(Flow::Return(value))
            }
            Stmt::If { branches, else_branch } => {
                for (condition, body) in branches {
                    if self.eval_expr(condition)?.truthy() {
                        return self.execute_block(body);
                    }
                }
                self.execute_block(else_branch)
            }
            Stmt::While { condition, body } => {
                let mut last = Value::Unit;
                while self.eval_expr(condition)?.truthy() {
                    match self.execute_block(body)? {
                        Flow::Value(value) => last = value,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                    }
                }
                Ok(Flow::Value(last))
            }
            Stmt::ForRange { name, start, end, body } => {
                let start_value = self.eval_expr(start)?;
                let end_value = self.eval_expr(end)?;
                let start = self.as_int(&start_value)?;
                let end = self.as_int(&end_value)?;
                let mut last = Value::Unit;
                for i in start..end {
                    self.scopes.push(HashMap::new());
                    self.define_var(name.clone(), TypeName::Int, Value::Int(i));
                    match self.execute_block(body)? {
                        Flow::Value(value) => last = value,
                        Flow::Return(value) => {
                            self.scopes.pop();
                            return Ok(Flow::Return(value));
                        }
                    }
                    self.scopes.pop();
                }
                Ok(Flow::Value(last))
            }
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::IntLiteral(v) => Ok(Value::Int(*v)),
            Expr::FloatLiteral(v) => Ok(Value::Float(*v)),
            Expr::DoubleLiteral(v) => Ok(Value::Double(*v)),
            Expr::StringLiteral(v) => Ok(Value::Str(v.clone())),
            Expr::BoolLiteral(v) => Ok(Value::Bool(*v)),
            Expr::Variable(name) => Ok(self.lookup_var(name)?.value.clone()),
            Expr::Call { name, args } => self.eval_call(name, args),
            Expr::Member { object, member } => {
                let obj = self.eval_expr(object)?;
                self.get_member(obj, member)
            }
            Expr::MethodCall { object, method, args } => {
                let obj = self.eval_expr(object)?;
                self.call_method(obj, method, args)
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_expr(expr)?;
                match op {
                    UnaryOp::Neg => match value {
                        Value::Int(v) => Ok(Value::Int(-v)),
                        Value::Float(v) => Ok(Value::Float(-v)),
                        Value::Double(v) => Ok(Value::Double(-v)),
                        _ => Err("unary '-' expects numeric value".to_string()),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!value.truthy())),
                }
            }
            Expr::Binary { left, op, right } => {
                let lhs = self.eval_expr(left)?;
                let rhs = self.eval_expr(right)?;
                self.eval_binary(lhs, *op, rhs)
            }
        }
    }

    fn eval_call(&mut self, name: &str, args: &[Expr]) -> Result<Value, String> {
        if self.classes.contains_key(name) {
            return self.instantiate_class(name, args);
        }

        match name {
            "print" => self.eval_print(args, false),
            "println" => self.eval_print(args, true),
            "input" => self.eval_input(args),
            "type" => self.eval_typeof(args),
            "str" => self.eval_cast(args, &TypeName::Str),
            "int" => self.eval_cast(args, &TypeName::Int),
            "float" => self.eval_cast(args, &TypeName::Float),
            "double" => self.eval_cast(args, &TypeName::Double),
            _ => {
                let function = self
                    .functions
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("unknown function: {name}"))?;
                self.call_function(&function, None, args)
            }
        }
    }

    fn eval_print(&mut self, args: &[Expr], newline: bool) -> Result<Value, String> {
        let mut values = Vec::new();
        let mut last = Value::Unit;
        for arg in args {
            let value = self.eval_expr(arg)?;
            last = value.clone();
            values.push(self.render_value(&value));
        }
        if newline {
            println!("{}", values.join(" "));
        } else {
            print!("{}", values.join(" "));
            let _ = io::stdout().flush();
        }
        Ok(last)
    }

    fn eval_input(&mut self, args: &[Expr]) -> Result<Value, String> {
        if args.len() > 1 {
            return Err("input() accepts zero or one argument".to_string());
        }
        if let Some(prompt) = args.first() {
            let prompt_value = self.eval_expr(prompt)?;
            print!("{}", self.render_value(&prompt_value));
            let _ = io::stdout().flush();
        }
        let mut buffer = String::new();
        io::stdin()
            .read_line(&mut buffer)
            .map_err(|err| format!("input error: {err}"))?;
        if buffer.ends_with('\n') {
            buffer.pop();
            if buffer.ends_with('\r') {
                buffer.pop();
            }
        }
        Ok(Value::Str(buffer))
    }

    fn eval_typeof(&mut self, args: &[Expr]) -> Result<Value, String> {
        if args.len() != 1 {
            return Err("type() expects exactly one argument".to_string());
        }
        let value = self.eval_expr(&args[0])?;
        let name = match value.ty() {
            Some(ty) => ty.keyword().to_string(),
            None => "unit".to_string(),
        };
        Ok(Value::Str(name))
    }

    fn eval_cast(&mut self, args: &[Expr], target: &TypeName) -> Result<Value, String> {
        if args.len() != 1 {
            return Err(format!("{}() expects exactly one argument", target.keyword()));
        }
        let value = self.eval_expr(&args[0])?;
        self.convert_value(value, target)
    }

    fn instantiate_class(&mut self, class_name: &str, args: &[Expr]) -> Result<Value, String> {
        let class = self.get_class(class_name)?.clone();
        let mut fields = HashMap::new();
        for field in self.collect_fields(class_name)? {
            let raw = self.eval_expr(&field.value)?;
            let converted = self.convert_value(raw, &field.ty)?;
            fields.insert(
                field.name.clone(),
                Variable {
                    ty: field.ty.clone(),
                    value: converted,
                },
            );
        }
        let object_id = self.objects.len();
        self.objects.push(ObjectInstance {
            fields,
        });
        let object = Value::Object {
            class_name: class.name.clone(),
            object_id,
        };

        if let Some((owner, init)) = self.lookup_method(class_name, "init")? {
            self.ensure_access(&owner, init.access)?;
            self.call_function(&init, Some(object.clone()), args)?;
        } else if !args.is_empty() {
            return Err(format!("class '{}' does not define init(...) but arguments were provided", class_name));
        }

        Ok(object)
    }

    fn call_method(&mut self, object: Value, method: &str, args: &[Expr]) -> Result<Value, String> {
        let (class_name, _) = self.object_parts(&object)?;
        let (owner, function) = self
            .lookup_method(class_name, method)?
            .ok_or_else(|| format!("unknown method '{}.{}'", class_name, method))?;
        self.ensure_access(&owner, function.access)?;
        self.call_function(&function, Some(object), args)
    }

    fn call_function(&mut self, function: &FunctionDef, self_value: Option<Value>, args: &[Expr]) -> Result<Value, String> {
        let mut arg_values = Vec::new();
        for arg in args {
            arg_values.push(self.eval_expr(arg)?);
        }

        self.scopes.push(HashMap::new());
        self.current_class.push(function.owner_class.clone());

        let mut arg_index = 0usize;
        for param in &function.params {
            if param.ty.is_none() && param.name == "self" {
                let self_value = self_value.clone().ok_or_else(|| format!("method '{}' requires self", function.name))?;
                let class_name = match &self_value {
                    Value::Object { class_name, .. } => class_name.clone(),
                    _ => return Err("self must be an object".to_string()),
                };
                self.define_var(param.name.clone(), TypeName::Named(class_name), self_value);
                continue;
            }

            let arg_value = arg_values
                .get(arg_index)
                .cloned()
                .ok_or_else(|| format!("function '{}' argument count mismatch", function.name))?;
            arg_index += 1;
            let ty = param.ty.clone().ok_or_else(|| format!("parameter '{}' must have a type", param.name))?;
            let converted = self.convert_value(arg_value, &ty)?;
            self.define_var(param.name.clone(), ty, converted);
        }

        if arg_index != arg_values.len() {
            self.scopes.pop();
            self.current_class.pop();
            return Err(format!("function '{}' argument count mismatch", function.name));
        }

        let result = match self.execute_block(&function.body)? {
            Flow::Value(value) => value,
            Flow::Return(value) => value,
        };

        self.current_class.pop();
        self.scopes.pop();
        Ok(result)
    }

    fn get_member(&mut self, object: Value, member: &str) -> Result<Value, String> {
        let (class_name, object_id) = self.object_parts(&object)?;
        let (owner, access) = self
            .lookup_field_owner_and_access(class_name, member)?
            .ok_or_else(|| format!("unknown field '{}.{}'", class_name, member))?;
        self.ensure_access(&owner, access)?;
        self.objects[object_id]
            .fields
            .get(member)
            .map(|var| var.value.clone())
            .ok_or_else(|| format!("unknown field '{}.{}'", class_name, member))
    }

    fn assign_member(&mut self, object: Value, member: &str, raw: Value) -> Result<(), String> {
        let (class_name, object_id) = self.object_parts(&object)?;
        let (owner, access) = self
            .lookup_field_owner_and_access(class_name, member)?
            .ok_or_else(|| format!("unknown field '{}.{}'", class_name, member))?;
        self.ensure_access(&owner, access)?;
        let target_ty = self.objects[object_id]
            .fields
            .get(member)
            .map(|var| var.ty.clone())
            .ok_or_else(|| format!("unknown field '{}.{}'", class_name, member))?;
        let converted = self.convert_value(raw, &target_ty)?;
        if let Some(field) = self.objects[object_id].fields.get_mut(member) {
            field.value = converted;
        }
        Ok(())
    }

    fn ensure_access(&self, owner_class: &str, access: AccessLevel) -> Result<(), String> {
        match access {
            AccessLevel::Public | AccessLevel::Default => Ok(()),
            AccessLevel::Private => {
                let current = self.current_class.last().and_then(|c| c.as_deref());
                if current == Some(owner_class) {
                    Ok(())
                } else {
                    Err(format!("cannot access restricted member of class '{owner_class}'"))
                }
            }
            AccessLevel::Protect => {
                let current = self.current_class.last().and_then(|c| c.as_deref());
                match current {
                    Some(current_class) if current_class == owner_class || self.is_subclass_of(current_class, owner_class) => Ok(()),
                    _ => Err(format!("cannot access restricted member of class '{owner_class}'")),
                }
            }
        }
    }

    fn get_class(&self, class_name: &str) -> Result<&ClassDefEval, String> {
        self.classes
            .get(class_name)
            .ok_or_else(|| format!("unknown class: {class_name}"))
    }

    fn collect_fields(&self, class_name: &str) -> Result<Vec<FieldDef>, String> {
        let mut out = Vec::new();
        let class = self.get_class(class_name)?;
        if let Some(base) = &class.base {
            out.extend(self.collect_fields(base)?);
        }
        out.extend(class.fields.clone());
        Ok(out)
    }

    fn lookup_method(&self, class_name: &str, method: &str) -> Result<Option<(String, FunctionDef)>, String> {
        let class = self.get_class(class_name)?;
        if let Some(found) = class.methods.get(method) {
            return Ok(Some((class.name.clone(), found.clone())));
        }
        if let Some(base) = &class.base {
            return self.lookup_method(base, method);
        }
        Ok(None)
    }

    fn lookup_field_owner_and_access(&self, class_name: &str, field: &str) -> Result<Option<(String, AccessLevel)>, String> {
        let class = self.get_class(class_name)?;
        if let Some(found) = class.fields.iter().find(|f| f.name == field) {
            return Ok(Some((class.name.clone(), found.access)));
        }
        if let Some(base) = &class.base {
            return self.lookup_field_owner_and_access(base, field);
        }
        Ok(None)
    }

    fn is_subclass_of(&self, child: &str, ancestor: &str) -> bool {
        if child == ancestor {
            return true;
        }
        let mut cursor = self.classes.get(child).and_then(|c| c.base.clone());
        while let Some(current) = cursor {
            if current == ancestor {
                return true;
            }
            cursor = self.classes.get(&current).and_then(|c| c.base.clone());
        }
        false
    }

    fn object_parts<'a>(&self, value: &'a Value) -> Result<(&'a str, usize), String> {
        match value {
            Value::Object { class_name, object_id } => Ok((class_name.as_str(), *object_id)),
            _ => Err("member access requires an object instance".to_string()),
        }
    }

    fn eval_binary(&self, lhs: Value, op: BinaryOp, rhs: Value) -> Result<Value, String> {
        match op {
            BinaryOp::Add => {
                if let (Value::Str(a), Value::Str(b)) = (&lhs, &rhs) {
                    return Ok(Value::Str(format!("{a}{b}")));
                }
                match common_numeric_type(&lhs, &rhs)? {
                    TypeName::Int => Ok(Value::Int(self.as_int(&lhs)? + self.as_int(&rhs)?)),
                    TypeName::Float => Ok(Value::Float(self.as_f32(&lhs)? + self.as_f32(&rhs)?)),
                    TypeName::Double => Ok(Value::Double(self.as_f64(&lhs)? + self.as_f64(&rhs)?)),
                    _ => Err("unsupported '+' operands".to_string()),
                }
            }
            BinaryOp::Sub => match common_numeric_type(&lhs, &rhs)? {
                TypeName::Int => Ok(Value::Int(self.as_int(&lhs)? - self.as_int(&rhs)?)),
                TypeName::Float => Ok(Value::Float(self.as_f32(&lhs)? - self.as_f32(&rhs)?)),
                TypeName::Double => Ok(Value::Double(self.as_f64(&lhs)? - self.as_f64(&rhs)?)),
                _ => Err("unsupported '-' operands".to_string()),
            },
            BinaryOp::Mul => {
                if let (Value::Str(s), Value::Int(n)) = (&lhs, &rhs) {
                    return Ok(Value::Str(s.repeat((*n).max(0) as usize)));
                }
                if let (Value::Int(n), Value::Str(s)) = (&lhs, &rhs) {
                    return Ok(Value::Str(s.repeat((*n).max(0) as usize)));
                }
                match common_numeric_type(&lhs, &rhs)? {
                    TypeName::Int => Ok(Value::Int(self.as_int(&lhs)? * self.as_int(&rhs)?)),
                    TypeName::Float => Ok(Value::Float(self.as_f32(&lhs)? * self.as_f32(&rhs)?)),
                    TypeName::Double => Ok(Value::Double(self.as_f64(&lhs)? * self.as_f64(&rhs)?)),
                    _ => Err("unsupported '*' operands".to_string()),
                }
            }
            BinaryOp::Div => match common_numeric_type(&lhs, &rhs)? {
                TypeName::Int => Ok(Value::Int(self.as_int(&lhs)? / self.as_int(&rhs)?)),
                TypeName::Float => Ok(Value::Float(self.as_f32(&lhs)? / self.as_f32(&rhs)?)),
                TypeName::Double => Ok(Value::Double(self.as_f64(&lhs)? / self.as_f64(&rhs)?)),
                _ => Err("unsupported '/' operands".to_string()),
            },
            BinaryOp::Mod => {
                match common_numeric_type(&lhs, &rhs)? {
                    TypeName::Int => Ok(Value::Int(self.as_int(&lhs)? % self.as_int(&rhs)?)),
                    _ => Err("'%' supports int only".to_string()),
                }
            }
            BinaryOp::Eq => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? == 0)),
            BinaryOp::NotEq => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? != 0)),
            BinaryOp::Lt => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? < 0)),
            BinaryOp::Lte => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? <= 0)),
            BinaryOp::Gt => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? > 0)),
            BinaryOp::Gte => Ok(Value::Bool(self.compare_values(&lhs, &rhs)? >= 0)),
        }
    }

    fn compare_values(&self, lhs: &Value, rhs: &Value) -> Result<i32, String> {
        match (lhs, rhs) {
            (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b) as i32),
            _ => match common_numeric_type(lhs, rhs)? {
                TypeName::Int => Ok(self.as_int(lhs)?.cmp(&self.as_int(rhs)?) as i32),
                TypeName::Float => {
                    let a = self.as_f32(lhs)?;
                    let b = self.as_f32(rhs)?;
                    Ok(if a < b { -1 } else if a > b { 1 } else { 0 })
                }
                TypeName::Double => {
                    let a = self.as_f64(lhs)?;
                    let b = self.as_f64(rhs)?;
                    Ok(if a < b { -1 } else if a > b { 1 } else { 0 })
                }
                TypeName::Bool => Ok(self.as_bool(lhs)?.cmp(&self.as_bool(rhs)?) as i32),
                TypeName::Str => Err("unreachable".to_string()),
                TypeName::Named(_) => Err("object comparison is not supported".to_string()),
            },
        }
    }

    fn render_value(&self, value: &Value) -> String {
        match value {
            Value::Float(v) => {
                if v.fract() == 0.0 { format!("{v:.1}") } else { v.to_string() }
            }
            Value::Double(v) => {
                if v.fract() == 0.0 { format!("{v:.1}") } else { v.to_string() }
            }
            _ => value.to_string(),
        }
    }

    fn convert_value(&self, value: Value, target: &TypeName) -> Result<Value, String> {
        match (value, target) {
            (Value::Int(v), TypeName::Int) => Ok(Value::Int(v)),
            (Value::Int(v), TypeName::Float) => Ok(Value::Float(v as f32)),
            (Value::Int(v), TypeName::Double) => Ok(Value::Double(v as f64)),
            (Value::Int(v), TypeName::Bool) => Ok(Value::Bool(v != 0)),
            (Value::Float(v), TypeName::Float) => Ok(Value::Float(v)),
            (Value::Float(v), TypeName::Double) => Ok(Value::Double(v as f64)),
            (Value::Float(v), TypeName::Int) => Ok(Value::Int(v as i64)),
            (Value::Double(v), TypeName::Double) => Ok(Value::Double(v)),
            (Value::Double(v), TypeName::Float) => Ok(Value::Float(v as f32)),
            (Value::Double(v), TypeName::Int) => Ok(Value::Int(v as i64)),
            (Value::Bool(v), TypeName::Bool) => Ok(Value::Bool(v)),
            (Value::Bool(v), TypeName::Int) => Ok(Value::Int(i64::from(v))),
            (Value::Str(v), TypeName::Str) => Ok(Value::Str(v)),
            (Value::Str(v), TypeName::Int) => v.trim().parse::<i64>().map(Value::Int).map_err(|_| format!("cannot convert '{v}' to int")),
            (Value::Str(v), TypeName::Float) => v.trim().parse::<f32>().map(Value::Float).map_err(|_| format!("cannot convert '{v}' to float")),
            (Value::Str(v), TypeName::Double) => v.trim().parse::<f64>().map(Value::Double).map_err(|_| format!("cannot convert '{v}' to double")),
            (Value::Object { class_name, object_id }, TypeName::Named(name)) if class_name == *name => {
                Ok(Value::Object { class_name, object_id })
            }
            (Value::Int(v), TypeName::Str) => Ok(Value::Str(v.to_string())),
            (Value::Float(v), TypeName::Str) => Ok(Value::Str(self.render_value(&Value::Float(v)))),
            (Value::Double(v), TypeName::Str) => Ok(Value::Str(self.render_value(&Value::Double(v)))),
            (Value::Bool(v), TypeName::Str) => Ok(Value::Str(if v { "True" } else { "False" }.to_string())),
            (source, target) => Err(format!(
                "cannot assign value of type '{}' to '{}'",
                source.ty().as_ref().map(|ty| ty.keyword()).unwrap_or("unit"),
                target.keyword()
            )),
        }
    }

    fn define_var(&mut self, name: String, ty: TypeName, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, Variable { ty, value });
        } else {
            self.globals.insert(name, Variable { ty, value });
        }
    }

    fn lookup_var(&self, name: &str) -> Result<&Variable, String> {
        for scope in self.scopes.iter().rev() {
            if let Some(var) = scope.get(name) {
                return Ok(var);
            }
        }
        self.globals.get(name).ok_or_else(|| format!("undefined variable: {name}"))
    }

    fn assign_var(&mut self, name: &str, value: Value) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(var) = scope.get_mut(name) {
                var.value = value;
                return Ok(());
            }
        }
        if let Some(var) = self.globals.get_mut(name) {
            var.value = value;
            return Ok(());
        }
        Err(format!("undefined variable: {name}"))
    }

    fn as_int(&self, value: &Value) -> Result<i64, String> {
        match value {
            Value::Int(v) => Ok(*v),
            Value::Bool(v) => Ok(i64::from(*v)),
            Value::Float(v) => Ok(*v as i64),
            Value::Double(v) => Ok(*v as i64),
            _ => Err("expected int-compatible value".to_string()),
        }
    }

    fn as_f32(&self, value: &Value) -> Result<f32, String> {
        match value {
            Value::Int(v) => Ok(*v as f32),
            Value::Float(v) => Ok(*v),
            Value::Double(v) => Ok(*v as f32),
            _ => Err("expected float-compatible value".to_string()),
        }
    }

    fn as_f64(&self, value: &Value) -> Result<f64, String> {
        match value {
            Value::Int(v) => Ok(*v as f64),
            Value::Float(v) => Ok(*v as f64),
            Value::Double(v) => Ok(*v),
            _ => Err("expected double-compatible value".to_string()),
        }
    }

    fn as_bool(&self, value: &Value) -> Result<bool, String> {
        match value {
            Value::Bool(v) => Ok(*v),
            Value::Int(v) => Ok(*v != 0),
            _ => Err("expected bool-compatible value".to_string()),
        }
    }

    fn execute_import(&mut self, path: &str) -> Result<(), String> {
        let resolved = PathBuf::from(path);
        let key = resolved.to_string_lossy().to_string();
        if self.imported_paths.contains(&key) {
            return Ok(());
        }

        let source = std::fs::read_to_string(&resolved)
            .map_err(|err| format!("failed to read import '{path}': {err}"))?;

        let mut lexer = Lexer::new(&source);
        let tokens = lexer
            .tokenize()
            .map_err(|err| format!("import lexer error in '{path}': {err}"))?;
        let mut parser = Parser::new(tokens);
        let program = parser
            .parse()
            .map_err(|err| format!("import parser error in '{path}': {err}"))?;

        self.imported_paths.insert(key);
        for stmt in &program.statements {
            match self.execute_stmt(stmt)? {
                Flow::Value(_) => {}
                Flow::Return(_) => return Err("return is not allowed at top-level imported file".to_string()),
            }
        }
        Ok(())
    }
}

fn common_numeric_type(lhs: &Value, rhs: &Value) -> Result<TypeName, String> {
    match (lhs.ty(), rhs.ty()) {
        (Some(TypeName::Double), _) | (_, Some(TypeName::Double)) => Ok(TypeName::Double),
        (Some(TypeName::Float), _) | (_, Some(TypeName::Float)) => Ok(TypeName::Float),
        (Some(TypeName::Int), Some(TypeName::Int)) => Ok(TypeName::Int),
        (Some(TypeName::Bool), Some(TypeName::Bool)) => Ok(TypeName::Bool),
        _ => Err("operands are not compatible".to_string()),
    }
}



