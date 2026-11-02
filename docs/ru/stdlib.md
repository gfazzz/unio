# API стандартной библиотеки: подмножество 0.1-draft

Версия 0.1-draft. Английская версия (основная): `docs/stdlib.md`. Основание: раздел 5 ТЗ, разделы 2–4 концепции.

Документ описывает часть стандартной библиотеки, которой пользуются эталонные программы: `docs/examples/todo.unio` и `examples/`. Эталонные программы вызывают только перечисленное здесь; это проверяет `spec/check_api.py`. Остальные модули требования Б-1 (`std.collections`, `std.math`, `std.decimal`, `std.path`, `std.process`, `std.time`, `std.random`, `std.log`, `std.test`) описываются на этапе 5 плана в той же форме.

Сигнатуры записаны на Unio без тел. Каждый блок кода проверяется по `spec/grammar.ebnf`. Комментарий `# permission: ...` называет полномочие манифеста (требование И-6), нужное функции; функции без такого комментария полномочия не нужны.

## Общие правила модулей

- Функция, которая может завершиться ошибкой, перечисляет конкретные типы ошибок (Б-2); `fails with Error` не используется.
- Операция, заблокированная песочницей, завершается объявленной ошибкой с `kind = PermissionDenied` (ПС-6). Функция, которая сообщает об отсутствии вместо ошибки (`files.exists`, `env.get`), сообщает о заблокированном объекте как об отсутствующем.
- У каждой аварийной операции есть безопасная форма (Я-18): `items[i]` и `items.get(i)`, `a / b` и `a.checked_divide(b)`.
- Методы с `var self` меняют значение и доступны только на `var`-месте (Я-11).

## Прелюдия

Доступна в каждом модуле без импорта.

```unio
interface Display:
    function to_text(self) returns Text

interface Error:
    function to_text(self) returns Text

interface Equal:
    function equals(self, other as Self) returns Bool

interface Order:
    function compare(self, other as Self) returns Int

interface Hash:
    function hash(self) returns Int

interface Add:
    function add(self, other as Self) returns Self

interface Subtract:
    function subtract(self, other as Self) returns Self

interface Multiply:
    function multiply(self, other as Self) returns Self

interface Divide:
    function divide(self, other as Self) returns Self

interface Resource:
    function release(var self)
```

`Self` внутри интерфейса обозначает реализующий его тип. У `Option[T]` варианты `some(value)` и `none`, у `Result[T, E]` — `ok(value)` и `error(value)`. Эти четыре имени видны всегда, и пользовательский код не может их объявить.

```unio
function print(text as Text)
function print_error(text as Text)
function assert_fail(message as Text) returns Never
function expect_fault(action as Function[()])
function is_same[T with Resource](a as T, b as T) returns Bool
```

`print` пишет текст и конец строки в стандартный вывод, `print_error` — в поток ошибок. `assert_fail` завершает тест или программу аварией; тип результата `Never` означает, что функция не возвращает управление, поэтому ею можно закончить строчный обработчик (Я-14а). `expect_fault` доступна только в блоках `test`.

## Встроенные типы

### Text

Текст — последовательность символов Unicode в UTF-8. Позиции и длины считаются в символах; `byte_length` считает байты.

```unio
function length(self) returns Int
function byte_length(self) returns Int
function is_empty(self) returns Bool
function contains(self, part as Text) returns Bool
function starts_with(self, prefix as Text) returns Bool
function count(self, part as Text) returns Int
function to_lower(self) returns Text
function lines(self) returns List[Text]
function split_whitespace(self) returns List[Text]
function to_int(self) returns Option[Int]
```

`count` считает непересекающиеся вхождения. `lines` делит по `\n` и `\r\n` и отбрасывает одну последнюю пустую строку. `to_int` принимает необязательный знак и десятичные цифры и возвращает `none` для всего остального, включая переполнение.

### List[T]

```unio
function length(self) returns Int
function is_empty(self) returns Bool
function get(self, index as Int) returns Option[T]
function find(self, predicate as Function[(T) -> Bool]) returns Option[T]
function find_index(self, predicate as Function[(T) -> Bool]) returns Option[Int]
function map[U](self, change as Function[(T) -> U]) returns List[U]
function enumerate(self, start as Int = 0) returns List[Tuple[Int, T]]
function max(self) returns Option[T]
function append(var self, item as T)
```

`items[index]` с индексом вне `0..length()` — авария; `get` — её безопасная форма. `max` требует `T with Order`. `==` сравнивает списки поэлементно.

### Map[K, V]

`K` реализует `Equal` и `Hash`.

```unio
function length(self) returns Int
function get(self, key as K) returns Option[V]
```

### Path

```unio
function Path(text as Text) returns Path
function join(self, part as Text) returns Path
function to_text(self) returns Text
```

`Path` реализует `Display`, `Equal` и `Hash`. `join` добавляет один компонент с разделителем платформы.

## std.args

```unio
function list() returns List[Text]
```

`list` возвращает аргументы программы без её имени. Флаги, значения по умолчанию и автоматическая справка (Б-4) добавляются на этапе 5.

## std.env

```unio
# permission: env (the name must be listed)
function get(name as Text) returns Option[Text]
function home_dir() returns Option[Path]
```

`get` возвращает `none` для переменной, которая не задана или не разрешена. `home_dir` спрашивает операционную систему, а не окружение, и полномочий не требует; возвращает `none`, если у пользователя нет домашнего каталога.

## std.files

```unio
enum IoErrorKind:
    NotFound
    PermissionDenied
    AlreadyExists
    InvalidData
    Other

error IoError:
    path as Path
    kind as IoErrorKind
    reason as Text
```

```unio
# permission: files.read
function exists(path as Path) returns Bool
# permission: files.read
function read_text(path as Path) returns Text fails with IoError
# permission: files.write
function write_text(path as Path, text as Text, durable as Bool = false) fails with IoError
# permission: files.write
function open_atomic(path as Path, durable as Bool = false) returns AtomicFile fails with IoError
```

`exists` возвращает `false` для пути, который не существует или недоступен. `read_text` завершается ошибкой вида `InvalidData`, если файл не в корректной UTF-8. `write_text` заменяет файл атомарно (Б-9): читатель видит либо старое, либо новое полное содержимое; `durable = true` дополнительно сбрасывает файл и каталог на носитель.

`AtomicFile` — ресурс. Его содержимое заменяет целевой файл только при нормальном завершении блока `using`; при ошибке или аварии целевой файл не меняется.

```unio
function write(var self, text as Text) fails with IoError
function write_line(var self, text as Text) fails with IoError
function release(var self)
```

## std.json

```unio
enum Value:
    Null
    Bool(value as Bool)
    Int(value as Int)
    Float(value as Float)
    Text(value as Text)
    Array(items as List[Value])
    Object(fields as Map[Text, Value])

error ParseError:
    line as Int
    column as Int
    message as Text

error DecodeError:
    path as Text
    message as Text
```

Имена вариантов `Value` совпадают со встроенными типами, поэтому они всегда пишутся с именем перечисления: `Value.Text("Anna")` (Я-6а).

```unio
function parse(text as Text) returns Value fails with ParseError
function encode[T](value as T, pretty as Bool = false) returns Text
function decode[T](value as Value) returns T fails with DecodeError
function kind_name(value as Value) returns Text
```

`parse` читает число без дробной части и экспоненты, которое помещается в `Int`, как `Value.Int`, любое другое — как `Value.Float`. `encode` и `decode` работают для `Value`, `Int`, `Float`, `Bool`, `Text`, `Path`, `Option`, `List`, `Map` с ключами `Text`, а также для структур и перечислений из них; компилятор проверяет это для `T` в месте вызова (Б-5). Структура кодируется объектом с именами полей; поле со значением по умолчанию при декодировании может отсутствовать. Вариант без полей кодируется своим именем, вариант с полями — объектом `{"Variant": {поля}}`. `DecodeError.path` — путь через точку к несовпавшему значению, например `"2.title"`. `kind_name` возвращает `"null"`, `"bool"`, `"int"`, `"float"`, `"text"`, `"array"` или `"object"`.

## std.text

```unio
function join(parts as List[Text], separator as Text) returns Text
function split(text as Text, separator as Text) returns List[Text]
```

`split` с пустым разделителем — авария; `split("", ".")` возвращает `[""]`.
