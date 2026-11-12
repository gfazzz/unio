# Стандартная библиотека: 0.1-draft

Версия 0.1-draft. Английская версия (основная): [../stdlib.md](../stdlib.md). Основание: раздел 5 ТЗ, разделы 2–4 концепции.

Стандартная библиотека написана на Unio в [stdlib/](../../stdlib) и разделена на три слоя. Исходники — нормативный API: сигнатуры, документирующие комментарии и реализации. Всё, что язык может выразить, реализовано на Unio; только примитивы операционной системы и среды выполнения объявлены как `external function`, и только в файлах с пометкой `unsafe module` (Б-7, Б-10). Внешние функции реализует среда выполнения в [compiler/unio-runtime](../../compiler/unio-runtime) (83 функции, [runtime.md](runtime.md), раздел 7), и тест там падает, если объявление и привязка расходятся. Этот документ — обзор, написанный вручную; `unio doc`, который должен строить справочник по исходникам (Д-3), пока не существует. Что реализовано и проверено, по требованиям, — в [status.md](status.md).

Библиотека содержит то, чем пользуются эталонные программы и инструменты проекта. `tools/check_api.unio` проверяет в CI, что `docs/examples/`, `examples/`, `tools/` и сама библиотека не вызывают ничего, что в ней не объявлено (его эталон на Python, `spec/check_api.py`, даёт тот же результат), а `unio test` выполняет блоки `test` каждого модуля в CI на Linux, macOS и Windows. Остальных модулей требования Б-1 (`std.collections`, `std.math`, `std.decimal`, `std.path`, `std.time`, `std.random`, `std.log`) пока нет; они добавляются в спринте S7 (этап 5) в том же виде.

## Слои

| Слой | Что нужно | Каталог | Кто пользуется |
|---|---|---|---|
| `core` | ничего: ни аллокатора, ни GC, ни операционной системы | [stdlib/core/](../../stdlib/core) | любой профиль, включая системный (концепция, раздел 8.1) |
| `alloc` | аллокатор; в 0.1 это куча GC | [stdlib/alloc/](../../stdlib/alloc) | приложения и системный код, который предоставляет аллокатор |
| `std` | операционная система | [stdlib/std/](../../stdlib/std) | приложения |

Файл может пользоваться только своим слоем и предшествующими; `spec/check_api.py` и `tools/check_api.unio` проверяют это для импортов, функций прелюдии и методов. Имена модулей не содержат слоя: `stdlib/alloc/json.unio` — это `std.json`, `stdlib/std/files.unio` — `std.files`. Файлы `prelude.unio` и `builtin/` всех доступных слоёв вместе образуют прелюдию. Тип можно расширять в нескольких слоях: `Int` получает арифметику с проверкой в `core`, а текстовую форму, которая создаёт новый текст, — в `alloc`.

## Файлы

| Файл | Содержание | На Unio | Внешние примитивы |
|---|---|---|---|
| [core/prelude.unio](../../stdlib/core/prelude.unio) | `Display`, `Error`, `Equal`, `Order`, `Hash`, интерфейсы операторов, `Resource`, `Option`, `Result`, `min`, `max`, `Range`, `RangeIterator`, `Iterator`, `range`, `assert_fail`, `expect_fault`, `is_same` | интерфейсы, перечисления, `min`, `max`, курсоры диапазона | аварии, тождественность ресурсов |
| [core/builtin/int.unio](../../stdlib/core/builtin/int.unio), [float.unio](../../stdlib/core/builtin/float.unio), [bool.unio](../../stdlib/core/builtin/bool.unio), [option.unio](../../stdlib/core/builtin/option.unio) | Числа, `Bool`, `Option` | `checked_divide`, `Bool.to_text`, `is_some`, `is_none`, `OrderedFloat`, `Float.total_order` | арифметика с проверкой, `is_nan`, `is_finite`, канонический ключ Float |
| [alloc/prelude.unio](../../stdlib/alloc/prelude.unio) | `Range.to_list` | `to_list` | — |
| [alloc/builtin/text.unio](../../stdlib/alloc/builtin/text.unio) | Методы `Text` | поиск, разбиение, обрезка, `char_at`, `to_int`, `replace`, `lines` | длина, символы, срезы, `index_of`, регистр, `to_float` |
| [alloc/builtin/list.unio](../../stdlib/alloc/builtin/list.unio) | Методы `List[T]` | `find`, `map`, `filter`, `enumerate`, `contains`, `max`, `sorted`, `sorted_by`, `fold`, `partition`, `group_by`, `zip`, `chunks`, `windows`, `try_map/filter/fold`, `iterator`, `sorted_with` (сортировка слиянием) | длина, `get`, `append`, `remove_last` |
| [alloc/builtin/map.unio](../../stdlib/alloc/builtin/map.unio), [set.unio](../../stdlib/alloc/builtin/set.unio) | `Map[K, V]`, `Set[T]`, `List.to_set` | `contains`, `keys`, `values`, `union`, `intersection`, `difference` | хранение записей |
| [alloc/builtin/int.unio](../../stdlib/alloc/builtin/int.unio), [float.unio](../../stdlib/alloc/builtin/float.unio) | Текстовая форма чисел | — | `to_text` |
| [alloc/builtin/bytes.unio](../../stdlib/alloc/builtin/bytes.unio) | `Bytes` | `is_empty` | создание, `length`, `to_text` |
| [alloc/text.unio](../../stdlib/alloc/text.unio) | `std.text`: `join`, `split` | обе | `from_code_point` |
| [alloc/json.unio](../../stdlib/alloc/json.unio) | `std.json`: `Value`, `parse`, `encode`, `encode_checked`, `decode` | разбор и запись | преобразования между типом и `Value`, которые порождает компилятор |
| [alloc/errors.unio](../../stdlib/alloc/errors.unio) | `std.errors`: `ContextError`, `context` | структурированная причина | — |
| [alloc/test.unio](../../stdlib/alloc/test.unio) | `std.test`: `check_property`, `Counterexample` | генерация, уменьшение, данные воспроизведения | — |
| [std/prelude.unio](../../stdlib/std/prelude.unio) | `print`, `print_error` | обе | стандартный вывод и поток ошибок |
| [std/builtin/path.unio](../../stdlib/std/builtin/path.unio) | `Path` | компоненты, `name`, `extension`, `parent`, `join`, `normalize` | создание, текстовая форма, разделитель платформы |
| [std/args.unio](../../stdlib/std/args.unio) | `std.args`: аргументы программы | — | аргументы |
| [std/env.unio](../../stdlib/std/env.unio) | `std.env`: переменные, домашний каталог | — | обе функции |
| [std/files.unio](../../stdlib/std/files.unio) | `std.files`: потоки байтов, разрешение путей, каталоги, атомарная запись, копирование, переименование | `open_reader`, `FileReader.read_bytes/release`, `write_text`, `write_bytes`, `copy_file`, `create_directories`, `list_tree`, протокол фиксации `AtomicFile` | отдельные системные вызовы, `rename` без замены |
| [std/process.unio](../../stdlib/std/process.unio) | `std.process`: запуск программ без оболочки, с ограничением времени | — | `run` |

## Общие правила модулей

- Функция, которая может завершиться ошибкой, перечисляет конкретные типы ошибок (Б-2); `fails with Error` не используется.
- Операция, заблокированная песочницей, завершается объявленной ошибкой с `kind = PermissionDenied` (SB-6). Функция, которая сообщает об отсутствии вместо ошибки (`files.exists`, `env.get`), сообщает о заблокированном объекте как об отсутствующем.
- У каждой аварийной операции есть безопасная форма (Я-18): `items[i]` и `items.get(i)`, `a / b` и `a.checked_divide(b)`.
- Методы с `var self` меняют значение и доступны только на `var`-месте (Я-11).
- Комментарий функции, которой нужно полномочие манифеста (И-6), называет его: `Permission: files.read`.
- `a in b` вызывает `b.contains(a)`.
- Запись файла атомарна (Б-9). `AtomicFile` заменяет целевой файл только при явном `commit()`, потому что `release` не знает, как завершился его блок. `commit` записывает каждый шаг (`Writing`, `Closed`, `Replaced`, `Durable`), поэтому неудавшуюся фиксацию можно повторить, а `release` убирает только то, что ещё принадлежит файлу; неудавшийся сброс каталога после переименования сообщается как `DurabilityUnconfirmed` (Б-9c):

```unio
using file = try files.open_atomic(path):
    for line in report:
        try file.write_line(line)
    try file.commit()
```

## Контракты и примеры RFC 0001

Изменения относительно исходного 0.1-draft: [контракты](language-contracts.md) уточняют поведение и слои. Новые реализации и `test`-блоки написаны на Unio, и `unio test` их выполняет. `FileReader`, `files.resolve`, ключ представления Float (`total_order_key`), преобразования JSON, порождаемые компилятором, и `Error.diagnostic_code` реализованы интерпретатором, нативным кодом и средой выполнения. Преобразование JSON типа с приватным полем — E0046, а декодирование такой структуры во время выполнения завершается `DecodeError` и не заполняет ни одного поля (Б-17). Известные пробелы ([status.md](status.md)): явной нормализации Unicode из Б-18 нет; `std.test` содержит ядро property-тестов, но встраивание в запуск тестов и JSON-воспроизведение (И-12) не реализованы.

| Задача | Пример и контракт |
|---|---|
| Диапазон | `range(0, 100)` — core без аллокации; `.iterator()` создаёт курсор, `.next()` возвращает `Option`; `.to_list()` явно требует alloc |
| Снимок списка | `items.iterator()` сохраняет значение списка на момент создания; изменение исходного списка не меняет обход |
| Полный порядок Float | `values.sorted_by(function (x as Float) returns OrderedFloat: x.total_order())`; NaN равны и последние, оба нуля равны |
| Свёртка | `items.fold(0, function (sum as Int, x as Int) returns Int: sum + x)`; начальное значение возвращается для пустого списка |
| Разбиение/группировка | `items.partition(predicate)`, `items.group_by(key)` сохраняют порядок; ключ вычисляется один раз |
| Пары/блоки/окна | `a.zip(b)` требует равной длины; `items.chunks(2)` включает остаток; `items.windows(2)` — только полные окна; неверный размер даёт `none` |
| Ошибка callback | `try items.try_map(parse_item)`, `try items.try_filter(validate_item)`, `try items.try_fold(initial, combine)` прекращаются при первой ошибке |
| JSON | `try json.parse(text, max_depth = 64, max_bytes = 1048576)`; `try json.encode_checked(value, pretty = true)`; дубли ключей запрещены |
| Контекст ошибки | `errors.context("reading configuration", cause)` сохраняет типизированную причину; `cause.diagnostic_code()` не зависит от локализации |
| Свойства | `try test.check_property(generate, shrink, holds, seed = 7, cases = 100, max_shrinks = 500)` возвращает `none` либо воспроизводимый контрпример |
| Пути | `Path("link/../file").normalize()` сохраняет `..`; `try files.resolve(path)` обращается к ОС; `components()` сохраняет диск/UNC как единый якорь |

```unio
using reader = try files.open_reader(path):
    while true:
        match try reader.read_bytes(max_bytes = 65536):
            case some(chunk):
                consume(chunk)
            case none:
                break
```

Здесь `path` и `consume` предоставляет приложение. Пример показывает ограниченное чтение, не обещает готовую функцию `consume` библиотеки. EOF и `break` не закрывают ресурс до конца `using`. Полные поведенческие примеры — [examples/contracts.unio](../../examples/contracts.unio), остальные тесты находятся рядом с реализациями. Новые API имеют явные эффекты и concrete-ошибки; обобщённый `E with Error` подставляется конкретным типом вызывающего кода.
