use std::env;
use std::fs;
use std::path::Path;
use std::process;

use yuumi::{create_backend, BytecodeCompiler, Interpreter, Lexer, NativeBackendKind, Parser, Value, Vm};

#[derive(Debug, Clone, Copy)]
enum Engine {
    Interp,
    Vm,
    Native,
}

fn main() {
    let (engine, script_path) = parse_cli(env::args().skip(1));
    let source = match load_source(script_path.as_deref()) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("input error: {err}");
            process::exit(1);
        }
    };

    let mut lexer = Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(tokens) => tokens,
        Err(err) => {
            eprintln!("lexer error: {err}");
            process::exit(1);
        }
    };

    let mut parser = Parser::new(tokens);
    let program_ast = match parser.parse() {
        Ok(program_ast) => program_ast,
        Err(err) => {
            eprintln!("parser error: {err}");
            process::exit(1);
        }
    };

    let result = match engine {
        Engine::Interp => {
            let mut interpreter = Interpreter::new();
            interpreter.execute_program(&program_ast)
        }
        Engine::Vm => {
            let compiler = BytecodeCompiler::new();
            let program = match compiler.compile_program(&program_ast) {
                Ok(program) => program,
                Err(err) => {
                    eprintln!("compile error: {err}");
                    process::exit(1);
                }
            };
            Vm::new().run(&program)
        }
        Engine::Native => create_backend(NativeBackendKind::Cranelift).execute_program(&program_ast),
    };

    match result {
        Ok(value) => {
            // 스크립트 실행 모드에서는 스크립트가 만든 출력만 보여준다.
            if script_path.is_none() {
                println!("engine: {}\nsource: {source}\nresult: {}", engine_name(engine), format_result(value));
            }
        }
        Err(err) => {
            eprintln!("runtime error: {err}");
            process::exit(1);
        }
    }
}

fn parse_cli(args: impl Iterator<Item = String>) -> (Engine, Option<String>) {
    let mut engine = Engine::Interp;
    let mut script_path = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--engine=") {
            engine = match value {
                "interp" => Engine::Interp,
                "vm" => Engine::Vm,
                "native" => Engine::Native,
                other => {
                    eprintln!("unknown engine '{other}', fallback to interp");
                    Engine::Interp
                }
            };
            continue;
        }

        if script_path.is_none() {
            script_path = Some(arg);
        }
    }

    (engine, script_path)
}

fn load_source(script_path: Option<&str>) -> Result<String, String> {
    match script_path {
        Some(path) => {
            let ext = Path::new(path)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("");
            if ext != "yu" {
                return Err(format!("script extension must be .yu: {path}"));
            }

            fs::read_to_string(path).map_err(|err| format!("failed to read '{path}': {err}"))
        }
        None => Ok("int score = 30\nif score == 30:\n    print(score)\nelse:\n    print(0)\n".to_string()),
    }
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Interp => "interp",
        Engine::Vm => "vm",
        Engine::Native => "native",
    }
}

fn format_result(value: Value) -> String {
    value.to_string()
}
