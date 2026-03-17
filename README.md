# Yuumi

현재 `Yuumi`는 **native(Cranelift) 엔진 고정**으로 실행되는 정적 타입 스크립트 언어입니다.

## 실행

```bash
cargo run -- script.yu
```

- `--engine=...` 옵션은 무시됩니다.
- `.yu` 확장자 파일만 실행할 수 있습니다.
- 단순 수치/문자열 중심 코드는 JIT 경로를 사용하고, `def`/`class` 같은 고급 문법은 evaluator 경로로 실행됩니다.

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
b = 3        # 3.0 으로 변환됨
```

사용자 정의 클래스 타입 선언도 가능합니다.

```yu
class Box:
    public int value = 0

Box b = Box()
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

### 3) 함수 정의 (`def`)

```yu
public def add(int a, int b):
    return a + b

println(add(2, 3))
```

지원 사항:
- `def name(...):` 문법
- 들여쓰기 블록 본문
- `return expr`
- 파라미터 타입 명시 가능
- 메서드의 `self` 파라미터는 타입 없이 사용

### 4) 클래스 정의 (`class`)

```yu
class Counter:
    public int value = 0

    public def init(self, int start):
        self.value = start

    public def inc(self):
        self.value = self.value + 1
        return self.value

Counter c = Counter(10)
println(c.value)
println(c.inc())
```

지원 사항:
- `class Name:` 문법
- 필드 선언
- 메서드 선언
- 생성자 역할의 `init(self, ...)`
- 멤버 접근: `obj.field`
- 메서드 호출: `obj.method(...)`
- 멤버 대입: `self.value = ...`, `obj.value = ...`

### 5) 접근 제한자

지원 키워드:
- `public`
- `default`
- `private`
- `protect`

예:

```yu
class Box:
    public int open = 1
    private int secret = 2
    protect int hidden = 3

    public def init(self):
        return 0
```

현재 규칙:
- `public`, `default`: 외부 접근 가능
- `private`, `protect`: 같은 클래스 내부 메서드에서만 접근 가능

예:

```yu
class Box:
    private int secret = 7
    public def init(self):
        return 0

Box b = Box()
println(b.secret)   # runtime error
```

### 6) 제어문

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

### 7) 연산

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
println(type(10))        # int
println(type(3.0f))      # float
println(type(2.0))       # double
println(type(True))      # bool
println(type("x"))      # str
println(type(Box()))     # Box
```

### 타입 변환

```yu
println(str(10))            # 10
println(int("42"))         # 42
println(float("3"))        # 3.0
println(double("3"))       # 3.0
println(float(str(3)))      # 3.0
println(double(str(3)))     # 3.0
```

변환 실패 시 런타임 에러가 발생합니다.

예:

```yu
int x = int("abc")
# runtime error: cannot convert 'abc' to int
```

## 숫자/캐스팅 규칙 요약

- `double -> float` 변환은 허용됩니다. (정밀도 손실 가능)
- `int -> float`, `int -> double` 변환은 허용됩니다. (`a = 3` -> `3.0`)
- `str("3") -> float/double` 변환은 허용됩니다. 결과는 `3.0` 형태로 출력됩니다.
- `bool -> float/double` 직접 변환은 허용되지 않습니다.

## 리터럴 규칙

- `int`: `10`, `-3`
- `float`: `1.5f`, `3f`
- `double`: `1.5`, `3.0`
- `bool`: `True`, `False`
- `str`: `"text"`

## 현재 제한 사항

- 엔진 선택은 없습니다. 항상 native 진입점을 사용합니다.
- 클래스 상속은 없습니다.
- `protect`는 현재 `private`와 동일하게 동작합니다.
- `default`는 현재 `public`처럼 동작합니다.
- `%`(modulo) 연산자는 없습니다.
- `&T`, `&mut T` 빌려쓰기 선언은 문법은 남아 있지만 native/evaluator 경로에서 사실상 지원 대상이 아닙니다.

## 간단 예제

```yu
public def add(int a, int b):
    return a + b

class Box:
    public int value = 0
    private int secret = 1

    public def init(self, int start):
        self.value = start
        self.secret = start + 10

    public def inc(self):
        self.value = self.value + 1
        return self.value

println(add(2, 3))

Box b = Box(5)
println(type(b), b.value)
println(b.inc())
```
