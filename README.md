# yuumi

Rust로 작성한 Python-like 문법 실험 언어 런타임/컴파일러 골격입니다. 현재는 정적 타입 선언, 들여쓰기 기반 `if/elif/else`, 기본 내장 함수 `print(...)`, borrow 선언, 그리고 `Cranelift` 네이티브 백엔드 fast path를 포함합니다.

## Structure

- `src/lexer.rs`: 토큰화 (`int score = 30`, `==`, `&int`, `&mut int` 등)
- `src/keywords.rs`: 언어 키워드 테이블 (`if`, `for`, `while`, 타입 키워드)
- `src/builtins.rs`: 내장 함수 테이블 (`print`, `range`)
- `src/parser.rs`: 타입 AST/Program 구성, 비교 연산, 들여쓰기 블록 파싱
- `src/runtime.rs`: 값 표현(`Value`), 타입(`TypeName`), borrow 메타데이터
- `src/interpreter.rs`: AST 인터프리터, 타입 변환, borrow 검사
- `src/vm.rs`: 슬롯 기반 바이트코드 VM
- `src/codegen.rs`: `Cranelift` JIT 네이티브 백엔드, `LLVM` 예약 자리
- `src/main.rs`: `interp` / `vm` / `native` 실행 선택

## 현재 문법

```text
int score = 30
&int shared = score
bool ready = True

if score == 30:
    print(score)
else:
    print(0)
```

지원 기능:
- 정적 선언: `int name = ...`, `float name = ...`, `double name = ...`, `bool name = ...`, `str name = "..."`
- borrow 선언: `&int view = score`, `&mut int writer = score`
- 산술/비교 연산: `+ - * /`, `== != < <= > >=`
- 문자열: `str` 타입, 문자열 리터럴(`"text"`), 문자열 결합(`"a" + "b"`)
- 단항 연산: `-expr`, `not expr`
- 기본 내장 함수: `print(a, b, ...)`
- 들여쓰기 블록: `if / elif / else`
- 반복문: `while`, `for <name> in range(...)`

## 속도 우선 구조

- 프론트엔드: Rust (`lexer -> parser -> typed AST`)
- 인터프리터: 빠른 검증용이지만 문자열 기반 환경 대신 슬롯화된 구조를 사용
- VM: 인덱스 기반 글로벌 슬롯과 단순 opcode로 해시 조회를 최소화
- 네이티브 백엔드: `Cranelift` JIT 연결 완료, `LLVM`는 차후 연결용 자리 유지
- 메모리/소유권: borrow 선언 시 shared/mutable 충돌 검사, 슬롯 기반 참조 추적

## 엔진별 상태

| 엔진 | 상태 | 지원 범위 |
|---|---|---|
| `interp` | 안정적 | 현재 문법 대부분 |
| `vm` | 안정적 | 현재 문법 대부분 |
| `native` | 실험적 | 현재는 `owned int/bool`, 비교식, `if`, `print` 중심 |

## Quick Start

```bash
cargo run
cargo run -- --engine=vm
cargo run -- --engine=native
cargo run -- script.yu
cargo run -- --engine=vm script.yu
cargo run -- --engine=native script.yu
cargo test
```

`*.yu` example:

```text
int score = 30
if score == 30:
	print(score)
elif False:
	print(20)
else:
	print(10)

for i in range(3):
	print(i)

while False:
	print(0)
```

## 제한 사항

- `native` 엔진은 아직 `float/double/borrow` 전체를 네이티브 코드로 내리지 않습니다.
- `LLVM` 백엔드는 아직 placeholder 입니다.
- borrow는 MVP 수준의 shared/mutable 충돌 검사를 제공하며, 완전한 Rust borrow checker는 아닙니다.

