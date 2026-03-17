use std::collections::HashMap;
use std::io::{self, Write};

use cranelift_codegen::ir::{condcodes::IntCC, types, AbiParam, InstBuilder};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, default_libcall_names, Linkage, Module};

use crate::builtins::BuiltinFunction;
use crate::evaluator;
use crate::parser::{BinaryOp, Expr, Program, Stmt, UnaryOp};
use crate::runtime::{BindingMode, TypeName, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBackendKind {
    Llvm,
    Cranelift,
}

pub trait NativeBackend {
    fn execute_program(&self, program: &Program) -> Result<Value, String>;
}

pub struct LlvmBackend;

impl NativeBackend for LlvmBackend {
    fn execute_program(&self, _program: &Program) -> Result<Value, String> {
        Err("LLVM backend is reserved, use Cranelift for the current native fast path".to_string())
    }
}

pub struct CraneliftBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeValueKind {
    Unit,
    Int,
    Bool,
    Float,
    Double,
    Str,
}

impl NativeBackend for CraneliftBackend {
    fn execute_program(&self, program: &Program) -> Result<Value, String> {
        if requires_evaluator(program) {
            return evaluator::execute_program(program);
        }

        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|err| err.to_string())?;
        let flags = settings::Flags::new(flag_builder);
        let isa_builder = cranelift_native::builder().map_err(|err| err.to_string())?;
        let isa = isa_builder.finish(flags).map_err(|err| err.to_string())?;

        let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
        jit_builder.symbol("print_i64", print_i64 as *const u8);
        jit_builder.symbol("print_bool", print_bool as *const u8);
        jit_builder.symbol("print_str", print_str as *const u8);
        jit_builder.symbol("str_concat", str_concat as *const u8);
        jit_builder.symbol("str_repeat", str_repeat as *const u8);
        jit_builder.symbol("print_space", print_space as *const u8);
        jit_builder.symbol("print_newline", print_newline as *const u8);
        jit_builder.symbol("read_input", read_input as *const u8);
        jit_builder.symbol("print_f32", print_f32 as *const u8);
        jit_builder.symbol("print_f64", print_f64 as *const u8);
        jit_builder.symbol("cast_to_str_int", cast_to_str_int as *const u8);
        jit_builder.symbol("cast_to_str_bool", cast_to_str_bool as *const u8);
        jit_builder.symbol("cast_to_str_f32", cast_to_str_f32 as *const u8);
        jit_builder.symbol("cast_to_str_f64", cast_to_str_f64 as *const u8);
        jit_builder.symbol("cast_to_int_str", cast_to_int_str as *const u8);
        jit_builder.symbol("cast_to_int_f32", cast_to_int_f32 as *const u8);
        jit_builder.symbol("cast_to_int_f64", cast_to_int_f64 as *const u8);
        jit_builder.symbol("cast_to_f32_int", cast_to_f32_int as *const u8);
        jit_builder.symbol("cast_to_f32_str", cast_to_f32_str as *const u8);
        jit_builder.symbol("cast_to_f32_f64", cast_to_f32_f64 as *const u8);
        jit_builder.symbol("cast_to_f64_int", cast_to_f64_int as *const u8);
        jit_builder.symbol("cast_to_f64_str", cast_to_f64_str as *const u8);
        jit_builder.symbol("cast_to_f64_f32", cast_to_f64_f32 as *const u8);

        let mut module = JITModule::new(jit_builder);

        let mut signature = module.make_signature();
        signature.returns.push(AbiParam::new(types::I64));
        let func_id = module
            .declare_function("yuumi_main", Linkage::Export, &signature)
            .map_err(|err| err.to_string())?;

        let mut ctx = module.make_context();
        ctx.func.signature = signature;
        let mut func_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut func_ctx);

        let entry = builder.create_block();
        builder.switch_to_block(entry);

        let mut codegen = NativeCodegen::new(&mut builder, &mut module);
        let (result_var, result_kind) = codegen.compile_program(program)?;
        builder.seal_all_blocks();
        let result = builder.use_var(result_var);
        builder.ins().return_(&[result]);
        builder.finalize();

        module
            .define_function(func_id, &mut ctx)
            .map_err(|err| err.to_string())?;
        module.clear_context(&mut ctx);
        module.finalize_definitions().map_err(|err| err.to_string())?;

        let code = module.get_finalized_function(func_id);
        LAST_ERROR.with(|e| *e.borrow_mut() = None);
        let compiled = unsafe { std::mem::transmute::<*const u8, fn() -> i64>(code) };
        let raw = compiled();
        if let Some(err) = LAST_ERROR.with(|e| e.borrow_mut().take()) {
            STRING_ARENA.with(|a| a.borrow_mut().clear());
            return Err(err);
        }
        STRING_ARENA.with(|a| a.borrow_mut().clear());
        Ok(match result_kind {
            NativeValueKind::Unit | NativeValueKind::Str | NativeValueKind::Float | NativeValueKind::Double => {
                Value::Unit
            }
            NativeValueKind::Int => Value::Int(raw),
            NativeValueKind::Bool => Value::Bool(raw != 0),
        })
    }
}

fn requires_evaluator(program: &Program) -> bool {
    program.statements.iter().any(stmt_requires_evaluator)
}

fn stmt_requires_evaluator(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Import { .. }
        |
        Stmt::FunctionDef { .. }
        | Stmt::ClassDef { .. }
        | Stmt::MemberAssign { .. }
        | Stmt::Return(_) => true,
        Stmt::Expr(expr) => expr_requires_evaluator(expr),
        Stmt::VarDecl { ty, value, .. } => matches!(ty, TypeName::Named(_)) || expr_requires_evaluator(value),
        Stmt::Assign { value, .. } => expr_requires_evaluator(value),
        Stmt::Swap { .. } => false,
        Stmt::If { branches, else_branch } => {
            branches.iter().any(|(cond, body)| expr_requires_evaluator(cond) || body.iter().any(stmt_requires_evaluator))
                || else_branch.iter().any(stmt_requires_evaluator)
        }
        Stmt::While { condition, body } => expr_requires_evaluator(condition) || body.iter().any(stmt_requires_evaluator),
        Stmt::ForRange { start, end, body, .. } => {
            expr_requires_evaluator(start) || expr_requires_evaluator(end) || body.iter().any(stmt_requires_evaluator)
        }
    }
}

fn expr_requires_evaluator(expr: &Expr) -> bool {
    match expr {
        Expr::Member { .. } | Expr::MethodCall { .. } => true,
        Expr::Call { args, .. } => args.iter().any(expr_requires_evaluator),
        Expr::Unary { expr, .. } => expr_requires_evaluator(expr),
        Expr::Binary { left, right, .. } => expr_requires_evaluator(left) || expr_requires_evaluator(right),
        _ => false,
    }
}

pub fn create_backend(kind: NativeBackendKind) -> Box<dyn NativeBackend> {
    match kind {
        NativeBackendKind::Llvm => Box::new(LlvmBackend),
        NativeBackendKind::Cranelift => Box::new(CraneliftBackend),
    }
}

extern "C" fn print_i64(value: i64) {
    print!("{value}");
}

extern "C" fn print_bool(value: i64) {
    print!("{}", if value == 0 { "False" } else { "True" });
}

extern "C" fn print_str(ptr: i64) {
    let cstr = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) };
    print!("{}", cstr.to_string_lossy());
}

thread_local! {
    static STRING_ARENA: std::cell::RefCell<Vec<Box<[u8]>>> = std::cell::RefCell::new(Vec::new());
    static LAST_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(message: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(message));
}

fn arena_alloc(mut bytes: Vec<u8>) -> i64 {
    bytes.push(0); // null-terminate
    let boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_ptr() as i64;
    STRING_ARENA.with(|a| a.borrow_mut().push(boxed));
    ptr
}

fn kind_name(kind: NativeValueKind) -> &'static str {
    match kind {
        NativeValueKind::Unit => "unit",
        NativeValueKind::Int => "int",
        NativeValueKind::Bool => "bool",
        NativeValueKind::Float => "float",
        NativeValueKind::Double => "double",
        NativeValueKind::Str => "str",
    }
}

fn format_f32_for_display(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn format_f64_for_display(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

extern "C" fn str_concat(ptr1: i64, ptr2: i64) -> i64 {
    let s1 = unsafe { std::ffi::CStr::from_ptr(ptr1 as *const i8) }.to_string_lossy();
    let s2 = unsafe { std::ffi::CStr::from_ptr(ptr2 as *const i8) }.to_string_lossy();
    arena_alloc(format!("{s1}{s2}").into_bytes())
}

extern "C" fn str_repeat(ptr: i64, count: i64) -> i64 {
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    let n = if count < 0 { 0 } else { count as usize };
    arena_alloc(s.repeat(n).into_bytes())
}

extern "C" fn print_space() {
    print!(" ");
}

extern "C" fn print_newline() {
    println!();
}

extern "C" fn read_input() -> i64 {
    let _ = io::stdout().flush();
    let mut buffer = String::new();
    if io::stdin().read_line(&mut buffer).is_err() {
        return arena_alloc(Vec::new());
    }

    if buffer.ends_with('\n') {
        buffer.pop();
        if buffer.ends_with('\r') {
            buffer.pop();
        }
    }

    arena_alloc(buffer.into_bytes())
}

extern "C" fn print_f32(bits: i64) {
    let value = f32::from_bits(bits as u32);
    print!("{}", format_f32_for_display(value));
}

extern "C" fn print_f64(bits: i64) {
    let value = f64::from_bits(bits as u64);
    print!("{}", format_f64_for_display(value));
}

extern "C" fn cast_to_str_int(value: i64) -> i64 {
    arena_alloc(value.to_string().into_bytes())
}

extern "C" fn cast_to_str_bool(value: i64) -> i64 {
    arena_alloc((if value == 0 { "False" } else { "True" }).as_bytes().to_vec())
}

extern "C" fn cast_to_str_f32(bits: i64) -> i64 {
    arena_alloc(format_f32_for_display(f32::from_bits(bits as u32)).into_bytes())
}

extern "C" fn cast_to_str_f64(bits: i64) -> i64 {
    arena_alloc(format_f64_for_display(f64::from_bits(bits as u64)).into_bytes())
}

extern "C" fn cast_to_int_str(ptr: i64) -> i64 {
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    match s.trim().parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            set_last_error(format!("cannot convert '{s}' to int"));
            0
        }
    }
}

extern "C" fn cast_to_int_f32(bits: i64) -> i64 {
    f32::from_bits(bits as u32) as i64
}

extern "C" fn cast_to_int_f64(bits: i64) -> i64 {
    f64::from_bits(bits as u64) as i64
}

extern "C" fn cast_to_f32_int(value: i64) -> i64 {
    (value as f32).to_bits() as i64
}

extern "C" fn cast_to_f32_str(ptr: i64) -> i64 {
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    match s.trim().parse::<f32>() {
        Ok(v) => v.to_bits() as i64,
        Err(_) => {
            set_last_error(format!("cannot convert '{s}' to float"));
            0
        }
    }
}

extern "C" fn cast_to_f32_f64(bits: i64) -> i64 {
    (f64::from_bits(bits as u64) as f32).to_bits() as i64
}

extern "C" fn cast_to_f64_int(value: i64) -> i64 {
    (value as f64).to_bits() as i64
}

extern "C" fn cast_to_f64_str(ptr: i64) -> i64 {
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
    match s.trim().parse::<f64>() {
        Ok(v) => v.to_bits() as i64,
        Err(_) => {
            set_last_error(format!("cannot convert '{s}' to double"));
            0
        }
    }
}

extern "C" fn cast_to_f64_f32(bits: i64) -> i64 {
    (f32::from_bits(bits as u32) as f64).to_bits() as i64
}

struct NativeCodegen<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    module: &'a mut JITModule,
    next_var: usize,
    vars: HashMap<String, (Variable, TypeName)>,
    print_i64: Option<cranelift_module::FuncId>,
    print_bool: Option<cranelift_module::FuncId>,
    print_f32: Option<cranelift_module::FuncId>,
    print_f64: Option<cranelift_module::FuncId>,
    print_str: Option<cranelift_module::FuncId>,
    str_concat: Option<cranelift_module::FuncId>,
    str_repeat: Option<cranelift_module::FuncId>,
    print_space: Option<cranelift_module::FuncId>,
    print_newline: Option<cranelift_module::FuncId>,
    read_input: Option<cranelift_module::FuncId>,
    cast_to_str_int: Option<cranelift_module::FuncId>,
    cast_to_str_bool: Option<cranelift_module::FuncId>,
    cast_to_str_f32: Option<cranelift_module::FuncId>,
    cast_to_str_f64: Option<cranelift_module::FuncId>,
    cast_to_int_str: Option<cranelift_module::FuncId>,
    cast_to_int_f32: Option<cranelift_module::FuncId>,
    cast_to_int_f64: Option<cranelift_module::FuncId>,
    cast_to_f32_int: Option<cranelift_module::FuncId>,
    cast_to_f32_str: Option<cranelift_module::FuncId>,
    cast_to_f32_f64: Option<cranelift_module::FuncId>,
    cast_to_f64_int: Option<cranelift_module::FuncId>,
    cast_to_f64_str: Option<cranelift_module::FuncId>,
    cast_to_f64_f32: Option<cranelift_module::FuncId>,
}

impl<'a, 'b> NativeCodegen<'a, 'b> {
    fn new(builder: &'a mut FunctionBuilder<'b>, module: &'a mut JITModule) -> Self {
        Self {
            builder,
            module,
            next_var: 0,
            vars: HashMap::new(),
            print_i64: None,
            print_bool: None,
            print_f32: None,
            print_f64: None,
            print_str: None,
            str_concat: None,
            str_repeat: None,
            print_space: None,
            print_newline: None,
            read_input: None,
            cast_to_str_int: None,
            cast_to_str_bool: None,
            cast_to_str_f32: None,
            cast_to_str_f64: None,
            cast_to_int_str: None,
            cast_to_int_f32: None,
            cast_to_int_f64: None,
            cast_to_f32_int: None,
            cast_to_f32_str: None,
            cast_to_f32_f64: None,
            cast_to_f64_int: None,
            cast_to_f64_str: None,
            cast_to_f64_f32: None,
        }
    }

    fn compile_program(&mut self, program: &Program) -> Result<(Variable, NativeValueKind), String> {
        let last = self.alloc_var(types::I64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.def_var(last, zero);
        let mut last_kind = NativeValueKind::Unit;

        for stmt in &program.statements {
            let (value, kind) = self.compile_stmt(stmt)?;
            self.builder.def_var(last, value);
            last_kind = kind;
        }

        Ok((last, last_kind))
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        match stmt {
            Stmt::Import { .. }
            |
            Stmt::FunctionDef { .. } | Stmt::ClassDef { .. } | Stmt::MemberAssign { .. } | Stmt::Return(_) => {
                Err("advanced syntax is handled by evaluator fallback".to_string())
            }
            Stmt::Expr(expr) => self.compile_expr(expr),
            Stmt::VarDecl { binding, name, ty, value } => {
                if *binding != BindingMode::Owned {
                    return Err("native backend currently supports owned bindings only".to_string());
                }
                let (expr_value, expr_ty) = self.compile_expr(value)?;
                let var = self.alloc_var(types::I64);
                let stored = self.coerce(expr_value, expr_ty, ty.clone())?;
                self.builder.def_var(var, stored);
                self.vars.insert(name.clone(), (var, ty.clone()));
                Ok((self.builder.ins().iconst(types::I64, 0), NativeValueKind::Unit))
            }
            Stmt::Assign { name, value } => {
                let (var, ty) = self.vars.get(name).cloned()
                    .ok_or_else(|| format!("undefined variable: {name}"))?;
                let (expr_value, expr_ty) = self.compile_expr(value)?;
                let stored = self.coerce(expr_value, expr_ty, ty)?;
                self.builder.def_var(var, stored);
                Ok((stored, NativeValueKind::Unit))
            }
            Stmt::Swap { left, right } => {
                if left.len() != right.len() {
                    return Err("swap requires same-length tuples".to_string());
                }

                // Snapshot RHS variables first to preserve tuple-assignment semantics.
                let mut rhs_values = Vec::with_capacity(right.len());
                let mut rhs_types = Vec::with_capacity(right.len());
                for rhs_name in right {
                    let (rhs_var, rhs_ty) = self
                        .vars
                        .get(rhs_name)
                        .cloned()
                        .ok_or_else(|| format!("undefined variable: {rhs_name}"))?;
                    rhs_values.push(self.builder.use_var(rhs_var));
                    rhs_types.push(rhs_ty);
                }

                for (index, left_name) in left.iter().enumerate() {
                    let (left_var, left_ty) = self
                        .vars
                        .get(left_name)
                        .cloned()
                        .ok_or_else(|| format!("undefined variable: {left_name}"))?;
                    let rhs_ty = rhs_types[index].clone();
                    if left_ty != rhs_ty {
                        return Err(format!(
                            "swap requires same type at position {}: '{}' is '{}', RHS is '{}'",
                            index,
                            left_name,
                            left_ty.keyword(),
                            rhs_ty.keyword()
                        ));
                    }
                    self.builder.def_var(left_var, rhs_values[index]);
                }

                Ok((self.builder.ins().iconst(types::I64, 0), NativeValueKind::Unit))
            }
            Stmt::If { branches, else_branch } => self.compile_if(branches, else_branch),
            Stmt::While { condition, body } => self.compile_while(condition, body),
            Stmt::ForRange { name, start, end, body } => self.compile_for_range(name, start, end, body),
        }
    }

    fn compile_for_range(
        &mut self,
        name: &str,
        start: &Expr,
        end: &Expr,
        body: &[Stmt],
    ) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        let (start_value, start_kind) = self.compile_expr(start)?;
        let (end_value, end_kind) = self.compile_expr(end)?;
        let start_int = self.coerce(start_value, start_kind, TypeName::Int)?;
        let end_int = self.coerce(end_value, end_kind, TypeName::Int)?;

        let loop_var = self.alloc_var(types::I64);
        self.builder.def_var(loop_var, start_int);
        let end_var = self.alloc_var(types::I64);
        self.builder.def_var(end_var, end_int);

        let result_var = self.alloc_var(types::I64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.def_var(result_var, zero);

        let previous = self
            .vars
            .insert(name.to_string(), (loop_var, TypeName::Int));

        let header_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.ins().jump(header_block, &[]);

        self.builder.switch_to_block(header_block);
        let current = self.builder.use_var(loop_var);
        let limit = self.builder.use_var(end_var);
        let cond = self
            .builder
            .ins()
            .icmp(IntCC::SignedLessThan, current, limit);
        self.builder
            .ins()
            .brif(cond, body_block, &[], exit_block, &[]);

        self.builder.switch_to_block(body_block);
        let (body_value, body_kind) = self.compile_block(body)?;
        self.builder.def_var(result_var, body_value);
        let one = self.builder.ins().iconst(types::I64, 1);
        let loop_current = self.builder.use_var(loop_var);
        let next = self.builder.ins().iadd(loop_current, one);
        self.builder.def_var(loop_var, next);
        self.builder.ins().jump(header_block, &[]);

        self.builder.switch_to_block(exit_block);

        match previous {
            Some((var, ty)) => {
                self.vars.insert(name.to_string(), (var, ty));
            }
            None => {
                self.vars.remove(name);
            }
        }

        Ok((self.builder.use_var(result_var), body_kind))
    }

    fn compile_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
    ) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        let result_var = self.alloc_var(types::I64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.def_var(result_var, zero);

        let header_block = self.builder.create_block();
        let body_block   = self.builder.create_block();
        let exit_block   = self.builder.create_block();

        self.builder.ins().jump(header_block, &[]);

        // 조건 검사
        self.builder.switch_to_block(header_block);
        let (cond_value, cond_ty) = self.compile_expr(condition)?;
        let cond_bool = self.coerce(cond_value, cond_ty, TypeName::Bool)?;
        let zero = self.builder.ins().iconst(types::I64, 0);
        let cond_flag = self.builder.ins().icmp(IntCC::NotEqual, cond_bool, zero);
        self.builder.ins().brif(cond_flag, body_block, &[], exit_block, &[]);

        // 루프 본문
        self.builder.switch_to_block(body_block);
        let (body_value, body_kind) = self.compile_block(body)?;
        self.builder.def_var(result_var, body_value);
        self.builder.ins().jump(header_block, &[]);

        self.builder.switch_to_block(exit_block);
        Ok((self.builder.use_var(result_var), body_kind))
    }

    fn compile_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_branch: &[Stmt],
    ) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        let result_var = self.alloc_var(types::I64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.def_var(result_var, zero);
        let merge_block = self.builder.create_block();
        let mut result_kind = None;
        let mut next_block = None;

        for (index, (condition, body)) in branches.iter().enumerate() {
            let current_block = next_block.take().unwrap_or_else(|| self.builder.current_block().unwrap());
            if index > 0 {
                self.builder.switch_to_block(current_block);
            }

            let (cond_value, cond_ty) = self.compile_expr(condition)?;
            let cond_bool = self.coerce(cond_value, cond_ty, TypeName::Bool)?;
            let zero = self.builder.ins().iconst(types::I64, 0);
            let cond_flag = self.builder.ins().icmp(IntCC::NotEqual, cond_bool, zero);
            let then_block = self.builder.create_block();
            let fallthrough_block = self.builder.create_block();
            self.builder.ins().brif(cond_flag, then_block, &[], fallthrough_block, &[]);
            self.builder.seal_block(current_block);

            self.builder.switch_to_block(then_block);
            let (body_value, body_kind) = self.compile_block(body)?;
            if let Some(existing) = result_kind {
                if existing != body_kind {
                    return Err("native backend requires all if branches to return the same type".to_string());
                }
            } else {
                result_kind = Some(body_kind);
            }
            self.builder.def_var(result_var, body_value);
            self.builder.ins().jump(merge_block, &[]);
            self.builder.seal_block(then_block);
            next_block = Some(fallthrough_block);
        }

        let else_block = next_block.unwrap_or_else(|| self.builder.create_block());
        self.builder.switch_to_block(else_block);
        let (else_value, else_kind) = self.compile_block(else_branch)?;
        if let Some(existing) = result_kind {
            if existing != else_kind {
                return Err("native backend requires all if branches to return the same type".to_string());
            }
        } else {
            result_kind = Some(else_kind);
        }
        self.builder.def_var(result_var, else_value);
        self.builder.ins().jump(merge_block, &[]);
        self.builder.seal_block(else_block);

        self.builder.switch_to_block(merge_block);
        self.builder.seal_block(merge_block);
        Ok((self.builder.use_var(result_var), result_kind.unwrap_or(NativeValueKind::Unit)))
    }

    fn compile_block(&mut self, body: &[Stmt]) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        let mut last = self.builder.ins().iconst(types::I64, 0);
        let mut last_kind = NativeValueKind::Unit;
        for stmt in body {
            let (value, kind) = self.compile_stmt(stmt)?;
            last = value;
            last_kind = kind;
        }
        Ok((last, last_kind))
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        match expr {
            Expr::IntLiteral(value) => Ok((self.builder.ins().iconst(types::I64, *value), NativeValueKind::Int)),
            Expr::BoolLiteral(value) => Ok((self.builder.ins().iconst(types::I64, i64::from(*value)), NativeValueKind::Bool)),
            Expr::FloatLiteral(value) => Ok((
                self.builder.ins().iconst(types::I64, value.to_bits() as i64),
                NativeValueKind::Float,
            )),
            Expr::DoubleLiteral(value) => Ok((
                self.builder.ins().iconst(types::I64, value.to_bits() as i64),
                NativeValueKind::Double,
            )),
            Expr::StringLiteral(s) => {
                let ptr = self.intern_string(s)?;
                Ok((ptr, NativeValueKind::Str))
            }
            Expr::Variable(name) => {
                let (var, ty) = self
                    .vars
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("undefined variable: {name}"))?;
                Ok((
                    self.builder.use_var(var),
                    match ty {
                        TypeName::Int => NativeValueKind::Int,
                        TypeName::Bool => NativeValueKind::Bool,
                        TypeName::Float => NativeValueKind::Float,
                        TypeName::Double => NativeValueKind::Double,
                        TypeName::Str => NativeValueKind::Str,
                        TypeName::Named(_) => {
                            return Err("class/object values are handled by evaluator fallback".to_string())
                        }
                    },
                ))
            }
            Expr::Call { name, args } => self.compile_call(name, args),
            Expr::Member { .. } | Expr::MethodCall { .. } => {
                Err("member access is handled by evaluator fallback".to_string())
            }
            Expr::Unary { op, expr } => {
                let (value, ty) = self.compile_expr(expr)?;
                match op {
                    UnaryOp::Neg => {
                        if ty != NativeValueKind::Int {
                            return Err("native backend unary '-' only supports int".to_string());
                        }
                        Ok((self.builder.ins().ineg(value), NativeValueKind::Int))
                    }
                    UnaryOp::Not => {
                        let value = self.coerce(value, ty, TypeName::Bool)?;
                        let zero = self.builder.ins().iconst(types::I64, 0);
                        let flag = self.builder.ins().icmp(IntCC::Equal, value, zero);
                        Ok((self.builder.ins().uextend(types::I64, flag), NativeValueKind::Bool))
                    }
                }
            }
            Expr::Binary { left, op, right } => self.compile_binary(left, *op, right),
        }
    }

    fn compile_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        match BuiltinFunction::from_name(name) {
            Some(BuiltinFunction::TypeOf) => {
                if args.len() != 1 {
                    return Err("type() expects exactly one argument".to_string());
                }
                let (_, kind) = self.compile_expr(&args[0])?;
                let ty_name = match kind {
                    NativeValueKind::Unit => "unit",
                    NativeValueKind::Int => "int",
                    NativeValueKind::Bool => "bool",
                    NativeValueKind::Float => "float",
                    NativeValueKind::Double => "double",
                    NativeValueKind::Str => "str",
                };
                let ptr = self.intern_string(ty_name)?;
                Ok((ptr, NativeValueKind::Str))
            }
            Some(BuiltinFunction::StrCast) => {
                if args.len() != 1 {
                    return Err("str() expects exactly one argument".to_string());
                }
                let (value, kind) = self.compile_expr(&args[0])?;
                if kind == NativeValueKind::Str {
                    return Ok((value, NativeValueKind::Str));
                }
                let callee = match kind {
                    NativeValueKind::Int => self.import_cast_to_str_int()?,
                    NativeValueKind::Bool => self.import_cast_to_str_bool()?,
                    NativeValueKind::Float => self.import_cast_to_str_f32()?,
                    NativeValueKind::Double => self.import_cast_to_str_f64()?,
                    NativeValueKind::Unit => return Err("cannot convert unit to str".to_string()),
                    NativeValueKind::Str => unreachable!(),
                };
                let local = self.module.declare_func_in_func(callee, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok((self.builder.inst_results(call)[0], NativeValueKind::Str))
            }
            Some(BuiltinFunction::IntCast) => {
                if args.len() != 1 {
                    return Err("int() expects exactly one argument".to_string());
                }
                let (value, kind) = self.compile_expr(&args[0])?;
                match kind {
                    NativeValueKind::Int => Ok((value, NativeValueKind::Int)),
                    NativeValueKind::Bool => Ok((value, NativeValueKind::Int)),
                    NativeValueKind::Str => {
                        let callee = self.import_cast_to_int_str()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Int))
                    }
                    NativeValueKind::Float => {
                        let callee = self.import_cast_to_int_f32()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Int))
                    }
                    NativeValueKind::Double => {
                        let callee = self.import_cast_to_int_f64()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Int))
                    }
                    NativeValueKind::Unit => Err("cannot convert unit to int".to_string()),
                }
            }
            Some(BuiltinFunction::FloatCast) => {
                if args.len() != 1 {
                    return Err("float() expects exactly one argument".to_string());
                }
                let (value, kind) = self.compile_expr(&args[0])?;
                match kind {
                    NativeValueKind::Float => Ok((value, NativeValueKind::Float)),
                    NativeValueKind::Int => {
                        let callee = self.import_cast_to_f32_int()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Float))
                    }
                    NativeValueKind::Bool => Err("cannot convert bool to float directly; use float(str(value))".to_string()),
                    NativeValueKind::Str => {
                        let callee = self.import_cast_to_f32_str()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Float))
                    }
                    NativeValueKind::Double => {
                        let callee = self.import_cast_to_f32_f64()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Float))
                    }
                    NativeValueKind::Unit => Err("cannot convert unit to float".to_string()),
                }
            }
            Some(BuiltinFunction::DoubleCast) => {
                if args.len() != 1 {
                    return Err("double() expects exactly one argument".to_string());
                }
                let (value, kind) = self.compile_expr(&args[0])?;
                match kind {
                    NativeValueKind::Double => Ok((value, NativeValueKind::Double)),
                    NativeValueKind::Int => {
                        let callee = self.import_cast_to_f64_int()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Double))
                    }
                    NativeValueKind::Bool => Err("cannot convert bool to double directly; use double(str(value))".to_string()),
                    NativeValueKind::Str => {
                        let callee = self.import_cast_to_f64_str()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Double))
                    }
                    NativeValueKind::Float => {
                        let callee = self.import_cast_to_f64_f32()?;
                        let local = self.module.declare_func_in_func(callee, self.builder.func);
                        let call = self.builder.ins().call(local, &[value]);
                        Ok((self.builder.inst_results(call)[0], NativeValueKind::Double))
                    }
                    NativeValueKind::Unit => Err("cannot convert unit to double".to_string()),
                }
            }
            Some(BuiltinFunction::Input) => {
                if args.len() > 1 {
                    return Err("input() accepts zero or one argument".to_string());
                }

                if let Some(prompt) = args.first() {
                    let (value, ty) = self.compile_expr(prompt)?;
                    let callee = match ty {
                        NativeValueKind::Int => self.import_print_i64()?,
                        NativeValueKind::Bool => self.import_print_bool()?,
                        NativeValueKind::Float => self.import_print_f32()?,
                        NativeValueKind::Double => self.import_print_f64()?,
                        NativeValueKind::Str => self.import_print_str()?,
                        NativeValueKind::Unit => {
                            return Err("cannot print unit value in input prompt".to_string())
                        }
                    };
                    let local = self.module.declare_func_in_func(callee, self.builder.func);
                    self.builder.ins().call(local, &[value]);
                }

                let input_fn = self.import_read_input()?;
                let local = self.module.declare_func_in_func(input_fn, self.builder.func);
                let call = self.builder.ins().call(local, &[]);
                let value = self.builder.inst_results(call)[0];
                Ok((value, NativeValueKind::Str))
            }
            Some(BuiltinFunction::Print) | Some(BuiltinFunction::Println) => {
                let is_println = matches!(BuiltinFunction::from_name(name), Some(BuiltinFunction::Println));
                let mut last_value = self.builder.ins().iconst(types::I64, 0);
                let mut last_kind = NativeValueKind::Unit;

                for (index, arg) in args.iter().enumerate() {
                    if index > 0 {
                        let space = self.import_print_space()?;
                        let local = self.module.declare_func_in_func(space, self.builder.func);
                        self.builder.ins().call(local, &[]);
                    }

                    let (value, ty) = self.compile_expr(arg)?;
                    let callee = match ty {
                        NativeValueKind::Int => self.import_print_i64()?,
                        NativeValueKind::Bool => self.import_print_bool()?,
                        NativeValueKind::Float => self.import_print_f32()?,
                        NativeValueKind::Double => self.import_print_f64()?,
                        NativeValueKind::Str => self.import_print_str()?,
                        NativeValueKind::Unit => {
                            return Err("cannot print unit value in native backend".to_string())
                        }
                    };
                    let local = self.module.declare_func_in_func(callee, self.builder.func);
                    self.builder.ins().call(local, &[value]);
                    last_value = value;
                    last_kind = ty;
                }

                if is_println {
                    let newline = self.import_print_newline()?;
                    let local = self.module.declare_func_in_func(newline, self.builder.func);
                    self.builder.ins().call(local, &[]);
                }

                Ok((last_value, last_kind))
            }
            _ => Err(format!("unknown function: {name}")),
        }
    }

    fn compile_binary(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(cranelift_codegen::ir::Value, NativeValueKind), String> {
        let (lhs, lhs_ty) = self.compile_expr(left)?;
        let (rhs, rhs_ty) = self.compile_expr(right)?;

        match op {
            BinaryOp::Add => {
                // 문자열 연결: "a" + "b"
                if lhs_ty == NativeValueKind::Str && rhs_ty == NativeValueKind::Str {
                    let callee = self.import_str_concat()?;
                    let local = self.module.declare_func_in_func(callee, self.builder.func);
                    let call = self.builder.ins().call(local, &[lhs, rhs]);
                    let result = self.builder.inst_results(call)[0];
                    return Ok((result, NativeValueKind::Str));
                }
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend '+' supports int+int or str+str only".to_string());
                }
                Ok((self.builder.ins().iadd(lhs, rhs), NativeValueKind::Int))
            }
            BinaryOp::Mul => {
                // 문자열 반복: "a" * 3  or  3 * "a"
                if lhs_ty == NativeValueKind::Str && rhs_ty == NativeValueKind::Int {
                    let callee = self.import_str_repeat()?;
                    let local = self.module.declare_func_in_func(callee, self.builder.func);
                    let call = self.builder.ins().call(local, &[lhs, rhs]);
                    let result = self.builder.inst_results(call)[0];
                    return Ok((result, NativeValueKind::Str));
                }
                if lhs_ty == NativeValueKind::Int && rhs_ty == NativeValueKind::Str {
                    let callee = self.import_str_repeat()?;
                    let local = self.module.declare_func_in_func(callee, self.builder.func);
                    let call = self.builder.ins().call(local, &[rhs, lhs]); // (str, count)
                    let result = self.builder.inst_results(call)[0];
                    return Ok((result, NativeValueKind::Str));
                }
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend '*' supports int*int, str*int, or int*str only".to_string());
                }
                Ok((self.builder.ins().imul(lhs, rhs), NativeValueKind::Int))
            }
            BinaryOp::Sub => {
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend '-' supports int only".to_string());
                }
                Ok((self.builder.ins().isub(lhs, rhs), NativeValueKind::Int))
            }
            BinaryOp::Div => {
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend '/' supports int only".to_string());
                }
                Ok((self.builder.ins().sdiv(lhs, rhs), NativeValueKind::Int))
            }
            BinaryOp::Mod => {
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend '%' supports int only".to_string());
                }
                Ok((self.builder.ins().srem(lhs, rhs), NativeValueKind::Int))
            }
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
                if lhs_ty != rhs_ty {
                    return Err("native backend comparison requires matching operand types".to_string());
                }
                let cc = match op {
                    BinaryOp::Eq => IntCC::Equal,
                    BinaryOp::NotEq => IntCC::NotEqual,
                    BinaryOp::Lt => IntCC::SignedLessThan,
                    BinaryOp::Lte => IntCC::SignedLessThanOrEqual,
                    BinaryOp::Gt => IntCC::SignedGreaterThan,
                    BinaryOp::Gte => IntCC::SignedGreaterThanOrEqual,
                    _ => unreachable!(),
                };
                let flag = self.builder.ins().icmp(cc, lhs, rhs);
                Ok((self.builder.ins().uextend(types::I64, flag), NativeValueKind::Bool))
            }
        }
    }

    fn coerce(
        &mut self,
        value: cranelift_codegen::ir::Value,
        source: NativeValueKind,
        target: TypeName,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let target_kind = match target {
            TypeName::Int => NativeValueKind::Int,
            TypeName::Bool => NativeValueKind::Bool,
            TypeName::Float => NativeValueKind::Float,
            TypeName::Double => NativeValueKind::Double,
            TypeName::Str => NativeValueKind::Str,
            TypeName::Named(_) => {
                return Err("class/object coercion is handled by evaluator fallback".to_string())
            }
        };

        if source == target_kind {
            return Ok(value);
        }

        match (source, target_kind) {
            (NativeValueKind::Int, NativeValueKind::Bool) => {
                let zero = self.builder.ins().iconst(types::I64, 0);
                let flag = self.builder.ins().icmp(IntCC::NotEqual, value, zero);
                Ok(self.builder.ins().uextend(types::I64, flag))
            }
            (NativeValueKind::Bool, NativeValueKind::Int) => Ok(value),

            (NativeValueKind::Int, NativeValueKind::Float) => {
                let f = self.import_cast_to_f32_int()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Bool, NativeValueKind::Float) => {
                Err("cannot convert to float directly from bool".to_string())
            }
            (NativeValueKind::Double, NativeValueKind::Float) => {
                let f = self.import_cast_to_f32_f64()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Str, NativeValueKind::Float) => {
                let f = self.import_cast_to_f32_str()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }

            (NativeValueKind::Int, NativeValueKind::Double) => {
                let f = self.import_cast_to_f64_int()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Bool, NativeValueKind::Double) => {
                Err("cannot convert to double directly from bool".to_string())
            }
            (NativeValueKind::Float, NativeValueKind::Double) => {
                let f = self.import_cast_to_f64_f32()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Str, NativeValueKind::Double) => {
                let f = self.import_cast_to_f64_str()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }

            (NativeValueKind::Str, NativeValueKind::Int) => {
                let f = self.import_cast_to_int_str()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Float, NativeValueKind::Int) => {
                let f = self.import_cast_to_int_f32()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Double, NativeValueKind::Int) => {
                let f = self.import_cast_to_int_f64()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }

            (NativeValueKind::Int, NativeValueKind::Str) => {
                let f = self.import_cast_to_str_int()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Bool, NativeValueKind::Str) => {
                let f = self.import_cast_to_str_bool()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Float, NativeValueKind::Str) => {
                let f = self.import_cast_to_str_f32()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }
            (NativeValueKind::Double, NativeValueKind::Str) => {
                let f = self.import_cast_to_str_f64()?;
                let local = self.module.declare_func_in_func(f, self.builder.func);
                let call = self.builder.ins().call(local, &[value]);
                Ok(self.builder.inst_results(call)[0])
            }

            (NativeValueKind::Unit, _) => Err("cannot coerce unit value in native backend".to_string()),
            _ => Err(format!(
                "cannot convert value from '{}' to '{}'",
                kind_name(source),
                target.keyword()
            )),
        }
    }

    fn alloc_var(&mut self, ty: cranelift_codegen::ir::Type) -> Variable {
        let var = Variable::from_u32(self.next_var as u32);
        self.next_var += 1;
        self.builder.declare_var(var, ty);
        var
    }

    fn intern_string(&mut self, s: &str) -> Result<cranelift_codegen::ir::Value, String> {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0); // null-terminate
        let name = format!("__str_{}", self.next_var);
        self.next_var += 1;
        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| e.to_string())?;
        let mut data_desc = DataDescription::new();
        data_desc.define(bytes.into_boxed_slice());
        self.module.define_data(data_id, &data_desc).map_err(|e| e.to_string())?;
        let gv = self.module.declare_data_in_func(data_id, self.builder.func);
        Ok(self.builder.ins().global_value(types::I64, gv))
    }

    fn import_print_i64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_i64 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("print_i64", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.print_i64 = Some(id);
        Ok(id)
    }

    fn import_print_bool(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_bool { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("print_bool", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.print_bool = Some(id);
        Ok(id)
    }

    fn import_print_f32(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_f32 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("print_f32", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.print_f32 = Some(id);
        Ok(id)
    }

    fn import_print_f64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_f64 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("print_f64", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.print_f64 = Some(id);
        Ok(id)
    }

    fn import_print_str(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_str { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("print_str", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.print_str = Some(id);
        Ok(id)
    }

    fn import_str_concat(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.str_concat { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("str_concat", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.str_concat = Some(id);
        Ok(id)
    }

    fn import_str_repeat(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.str_repeat { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("str_repeat", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.str_repeat = Some(id);
        Ok(id)
    }

    fn import_cast_to_str_int(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_str_int { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_str_int", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_str_int = Some(id);
        Ok(id)
    }

    fn import_cast_to_str_bool(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_str_bool { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_str_bool", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_str_bool = Some(id);
        Ok(id)
    }

    fn import_cast_to_str_f32(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_str_f32 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_str_f32", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_str_f32 = Some(id);
        Ok(id)
    }

    fn import_cast_to_str_f64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_str_f64 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_str_f64", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_str_f64 = Some(id);
        Ok(id)
    }

    fn import_cast_to_int_str(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_int_str { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_int_str", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_int_str = Some(id);
        Ok(id)
    }

    fn import_cast_to_int_f32(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_int_f32 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_int_f32", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_int_f32 = Some(id);
        Ok(id)
    }

    fn import_cast_to_int_f64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_int_f64 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_int_f64", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_int_f64 = Some(id);
        Ok(id)
    }

    fn import_cast_to_f32_int(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f32_int { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f32_int", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f32_int = Some(id);
        Ok(id)
    }

    fn import_cast_to_f32_str(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f32_str { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f32_str", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f32_str = Some(id);
        Ok(id)
    }

    fn import_cast_to_f32_f64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f32_f64 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f32_f64", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f32_f64 = Some(id);
        Ok(id)
    }

    fn import_cast_to_f64_int(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f64_int { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f64_int", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f64_int = Some(id);
        Ok(id)
    }

    fn import_cast_to_f64_str(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f64_str { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f64_str", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f64_str = Some(id);
        Ok(id)
    }

    fn import_cast_to_f64_f32(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.cast_to_f64_f32 { return Ok(id); }
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = self.module.declare_function("cast_to_f64_f32", Linkage::Import, &sig).map_err(|e| e.to_string())?;
        self.cast_to_f64_f32 = Some(id);
        Ok(id)
    }

    fn import_print_space(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_space {
            return Ok(id);
        }
        let sig = self.module.make_signature();
        let id = self
            .module
            .declare_function("print_space", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.print_space = Some(id);
        Ok(id)
    }

    fn import_print_newline(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_newline {
            return Ok(id);
        }
        let sig = self.module.make_signature();
        let id = self
            .module
            .declare_function("print_newline", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.print_newline = Some(id);
        Ok(id)
    }

    fn import_read_input(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.read_input {
            return Ok(id);
        }
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("read_input", Linkage::Import, &sig)
            .map_err(|e| e.to_string())?;
        self.read_input = Some(id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::{CraneliftBackend, NativeBackend};
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::runtime::Value;

    #[test]
    fn runs_int_program_with_cranelift() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    7\nelse:\n    2\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let result = CraneliftBackend.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn print_returns_last_argument() {
        let mut lexer = Lexer::new("int score = 30\nif score == 30:\n    print(score)\nelse:\n    print(0)\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse().unwrap();

        let result = CraneliftBackend.execute_program(&program).unwrap();
        assert_eq!(result, Value::Int(30));
    }
}
