use std::collections::HashMap;

use cranelift_codegen::ir::{condcodes::IntCC, types, AbiParam, InstBuilder};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};

use crate::builtins::BuiltinFunction;
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
}

impl NativeBackend for CraneliftBackend {
    fn execute_program(&self, program: &Program) -> Result<Value, String> {
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
        let compiled = unsafe { std::mem::transmute::<*const u8, fn() -> i64>(code) };
        let raw = compiled();
        Ok(match result_kind {
            NativeValueKind::Unit => Value::Unit,
            NativeValueKind::Int => Value::Int(raw),
            NativeValueKind::Bool => Value::Bool(raw != 0),
        })
    }
}

pub fn create_backend(kind: NativeBackendKind) -> Box<dyn NativeBackend> {
    match kind {
        NativeBackendKind::Llvm => Box::new(LlvmBackend),
        NativeBackendKind::Cranelift => Box::new(CraneliftBackend),
    }
}

extern "C" fn print_i64(value: i64) {
    println!("{value}");
}

extern "C" fn print_bool(value: i64) {
    println!("{}", if value == 0 { "False" } else { "True" });
}

struct NativeCodegen<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    module: &'a mut JITModule,
    next_var: usize,
    vars: HashMap<String, (Variable, TypeName)>,
    print_i64: Option<cranelift_module::FuncId>,
    print_bool: Option<cranelift_module::FuncId>,
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
            Stmt::Expr(expr) => self.compile_expr(expr),
            Stmt::VarDecl {
                binding,
                name,
                ty,
                value,
            } => {
                if *binding != BindingMode::Owned {
                    return Err("native backend currently supports owned bindings only".to_string());
                }
                if matches!(ty, TypeName::Float | TypeName::Double | TypeName::Str) {
                    return Err("native backend currently supports int/bool declarations only".to_string());
                }
                let (expr_value, expr_ty) = self.compile_expr(value)?;
                let coerced = self.coerce(expr_value, expr_ty, *ty)?;
                let var = self.alloc_var(types::I64);
                self.builder.def_var(var, coerced);
                self.vars.insert(name.clone(), (var, *ty));
                Ok((self.builder.ins().iconst(types::I64, 0), NativeValueKind::Unit))
            }
            Stmt::If {
                branches,
                else_branch,
            } => self.compile_if(branches, else_branch),
            Stmt::While { .. } => Err("native backend currently does not support while yet".to_string()),
            Stmt::ForRange { .. } => Err("native backend currently does not support for-range yet".to_string()),
        }
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
            Expr::FloatLiteral(_) => Err("native backend currently supports int/bool expressions only".to_string()),
            Expr::DoubleLiteral(_) => Err("native backend currently supports int/bool expressions only".to_string()),
            Expr::StringLiteral(_) => Err("native backend currently does not support string expressions".to_string()),
            Expr::Variable(name) => {
                let (var, ty) = self
                    .vars
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("undefined variable: {name}"))?;
                Ok((
                    self.builder.use_var(var),
                    match ty {
                        TypeName::Int => NativeValueKind::Int,
                        TypeName::Bool => NativeValueKind::Bool,
                        TypeName::Float | TypeName::Double | TypeName::Str => {
                            return Err("native backend currently supports int/bool variables only".to_string())
                        }
                    },
                ))
            }
            Expr::Call { name, args } => self.compile_call(name, args),
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
            Some(BuiltinFunction::Print) | Some(BuiltinFunction::Println) => {
                let mut last_value = self.builder.ins().iconst(types::I64, 0);
                let mut last_kind = NativeValueKind::Unit;

                for arg in args {
                    let (value, ty) = self.compile_expr(arg)?;
                    let callee = match ty {
                        NativeValueKind::Int => self.import_print_i64()?,
                        NativeValueKind::Bool => self.import_print_bool()?,
                NativeValueKind::Unit => {
                    return Err("cannot print unit value in native backend".to_string())
                }
            };
            let local = self.module.declare_func_in_func(callee, self.builder.func);
            self.builder.ins().call(local, &[value]);
            last_value = value;
            last_kind = ty;
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
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                if lhs_ty != NativeValueKind::Int || rhs_ty != NativeValueKind::Int {
                    return Err("native backend arithmetic currently supports int only".to_string());
                }
                let value = match op {
                    BinaryOp::Add => self.builder.ins().iadd(lhs, rhs),
                    BinaryOp::Sub => self.builder.ins().isub(lhs, rhs),
                    BinaryOp::Mul => self.builder.ins().imul(lhs, rhs),
                    BinaryOp::Div => self.builder.ins().sdiv(lhs, rhs),
                    _ => unreachable!(),
                };
                Ok((value, NativeValueKind::Int))
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
        match (source, target) {
            (NativeValueKind::Int, TypeName::Int)
            | (NativeValueKind::Bool, TypeName::Bool)
            | (NativeValueKind::Int, TypeName::Bool)
            | (NativeValueKind::Bool, TypeName::Int) => {
                if target == TypeName::Bool {
                    let zero = self.builder.ins().iconst(types::I64, 0);
                    let flag = self.builder.ins().icmp(IntCC::NotEqual, value, zero);
                    Ok(self.builder.ins().uextend(types::I64, flag))
                } else {
                    Ok(value)
                }
            }
            (NativeValueKind::Unit, _) => Err("cannot coerce unit value in native backend".to_string()),
            _ => Err("native backend coercion currently supports int/bool only".to_string()),
        }
    }

    fn alloc_var(&mut self, ty: cranelift_codegen::ir::Type) -> Variable {
        let var = Variable::from_u32(self.next_var as u32);
        self.next_var += 1;
        self.builder.declare_var(var, ty);
        var
    }

    fn import_print_i64(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_i64 {
            return Ok(id);
        }

        let mut signature = self.module.make_signature();
        signature.params.push(AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("print_i64", Linkage::Import, &signature)
            .map_err(|err| err.to_string())?;
        self.print_i64 = Some(id);
        Ok(id)
    }

    fn import_print_bool(&mut self) -> Result<cranelift_module::FuncId, String> {
        if let Some(id) = self.print_bool {
            return Ok(id);
        }

        let mut signature = self.module.make_signature();
        signature.params.push(AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("print_bool", Linkage::Import, &signature)
            .map_err(|err| err.to_string())?;
        self.print_bool = Some(id);
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
