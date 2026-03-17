# Yuumi

현재 `Yuumi`는 **native(Cranelift) 엔진 고정**으로 실행되는 정적 타입 스크립트 언어입니다.

## 실행

```bash
cargo run -- script.yu
```

- `--engine=...` 옵션은 무시됩니다.
- `.yu` 확장자 파일만 실행할 수 있습니다.

## 현재 지원 문법

### 1) 변수 선언 / 재할당

```yu
int a = 10
float b = 1.5f
double c = 2.0
bool ok = True
str name = "yuumi"

a = 20
name = "mike"
```

### 2) 다중 변수 스왑 (파이썬 스타일)

```yu
int a = 10
int b = 20
int c = 30

a, b, c = c, b, a
```

- 좌/우 변수 개수는 같아야 합니다.
- 좌/우에 등장하는 변수 집합이 같아야 합니다.
- 중복 변수명은 허용하지 않습니다.
- 각 대입 위치에서 타입이 다르면 에러가 납니다.

### 3) 제어문

```yu
if a > 0:
    println("positive")
elif a == 0:
    println("zero")
else:
    println("negative")

while a < 5:
    a = a + 1

for i in range(3):
    println(i)

for i in range(2, 5):
    println(i)
```

### 4) 연산

- 산술: `+ - * /`
- 비교: `== != < <= > >=`
- 단항: `-x`, `not x`

문자열 특수 연산:

```yu
println("ab" + "cd")   # abcd
println("*" * 4)       # ****
println(3 * "ab")      # ababab
```

## 내장 함수

### 출력

```yu
print(10, 20, 30)      # 10 20 30  (개행 없음)
println(10, 20, 30)    # 10 20 30  (개행 있음)
```

- 콤마 인자는 공백으로 구분되어 출력됩니다.

### 입력

```yu
str name = input("name: ")
println("hello", name)
```

- `input()` 또는 `input(prompt)` 형태를 지원합니다.

### 타입 확인

```yu
println(type(10))      # int
println(type(3.0f))    # float
println(type(2.0))     # double
println(type(True))    # bool
println(type("x"))    # str
```

### 타입 변환

```yu
println(str(10))          # "10"
println(int("42"))       # 42
println(float("3"))      # 3.0
println(double("3"))     # 3.0
println(float(str(3)))    # 3.0
println(double(str(3)))   # 3.0
```

변환 실패 시 런타임 에러가 발생합니다.

예:

```yu
int x = int("abc")
# runtime error: cannot convert 'abc' to int
```

## 숫자/캐스팅 규칙 요약

- `double -> float` 변환은 허용됩니다. (정밀도 손실 가능)
- `int -> float`, `int -> double` 변환은 허용됩니다. (예: `a = 3` -> `3.0`)
- `str("3") -> float/double` 변환은 허용됩니다. 결과는 `3.0` 형태로 출력됩니다.
- `bool -> float/double` 직접 변환은 허용되지 않습니다.

## 리터럴 규칙

- `int`: `10`, `-3`
- `float`: `1.5f`, `3f`
- `double`: `1.5`, `3.0`
- `bool`: `True`, `False`
- `str`: `"text"`

## 현재 제한 사항

- 엔진은 native 단일 경로입니다.
- 사용자 정의 함수는 없습니다.
- `&T`, `&mut T` 빌려쓰기 선언은 문법은 존재하지만 native 실행에서 지원하지 않습니다.
- `%`(modulo) 연산자는 현재 없습니다.

## 간단 예제

```yu
str name = input("name: ")
float amount = float(name)
println("amount:", amount)

int a = 10
int b = 20
int c = 30
a, b, c = c, b, a
println(a, b, c)

println(type(amount), type(a), type(name))
```
