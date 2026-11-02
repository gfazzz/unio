# Стандартная библиотека: 0.1-draft

Версия 0.1-draft. Английская версия (основная): [../stdlib.md](../stdlib.md). Основание: раздел 5 ТЗ, разделы 2–4 концепции.

Стандартная библиотека написана на Unio в [stdlib/](../../stdlib) и разделена на три слоя. Исходники — нормативный API: сигнатуры, документирующие комментарии и реализации. Всё, что язык может выразить, реализовано на Unio; только примитивы операционной системы и среды выполнения объявлены как `external function`, и только в файлах с пометкой `unsafe module` (Б-7, Б-10). Этот документ — обзор; справочник по исходникам будет строить `unio doc` (Д-3).

Библиотека содержит то, чем пользуются эталонные программы и инструменты проекта. `spec/check_api.py` проверяет, что `docs/examples/`, `examples/`, `tools/` и сама библиотека не вызывают ничего, что в ней не объявлено. Остальные модули требования Б-1 (`std.collections`, `std.math`, `std.decimal`, `std.path`, `std.process`, `std.time`, `std.random`, `std.log`, `std.test`) добавляются на этапе 5 в том же виде.

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
| [core/prelude.unio](../../stdlib/core/prelude.unio) | `Display`, `Error`, `Equal`, `Order`, `Hash`, интерфейсы операторов, `Resource`, `Option`, `Result`, `min`, `max`, `assert_fail`, `expect_fault`, `is_same` | интерфейсы, перечисления, `min`, `max` | аварии, тождественность ресурсов |
| [core/builtin/int.unio](../../stdlib/core/builtin/int.unio), [float.unio](../../stdlib/core/builtin/float.unio), [bool.unio](../../stdlib/core/builtin/bool.unio), [option.unio](../../stdlib/core/builtin/option.unio) | Числа, `Bool`, `Option` | `checked_divide`, `Bool.to_text`, `is_some`, `is_none` | арифметика с проверкой, `is_nan`, `is_finite` |
| [alloc/prelude.unio](../../stdlib/alloc/prelude.unio) | `range` | `range` | — |
| [alloc/builtin/text.unio](../../stdlib/alloc/builtin/text.unio) | Методы `Text` | поиск, разбиение, обрезка, `char_at`, `to_int`, `replace`, `lines` | длина, символы, срезы, `index_of`, регистр, `to_float` |
| [alloc/builtin/list.unio](../../stdlib/alloc/builtin/list.unio) | Методы `List[T]` | `find`, `map`, `filter`, `enumerate`, `contains`, `max`, `sorted`, `sorted_by`, `sorted_with` (сортировка слиянием) | длина, `get`, `append`, `remove_last` |
| [alloc/builtin/map.unio](../../stdlib/alloc/builtin/map.unio), [set.unio](../../stdlib/alloc/builtin/set.unio) | `Map[K, V]`, `Set[T]`, `List.to_set` | `contains`, `keys`, `values`, `union`, `intersection`, `difference` | хранение записей |
| [alloc/builtin/int.unio](../../stdlib/alloc/builtin/int.unio), [float.unio](../../stdlib/alloc/builtin/float.unio) | Текстовая форма чисел | — | `to_text` |
| [alloc/text.unio](../../stdlib/alloc/text.unio) | `std.text`: `join`, `split` | обе | `from_code_point` |
| [alloc/json.unio](../../stdlib/alloc/json.unio) | `std.json`: `Value`, `parse`, `encode`, `decode` | разбор и запись | преобразования между типом и `Value`, которые порождает компилятор |
| [std/prelude.unio](../../stdlib/std/prelude.unio) | `print`, `print_error` | обе | стандартный вывод и поток ошибок |
| [std/builtin/path.unio](../../stdlib/std/builtin/path.unio) | `Path` | компоненты, `name`, `extension`, `parent`, `join`, `normalize` | создание, текстовая форма, разделитель платформы |
| [std/args.unio](../../stdlib/std/args.unio) | `std.args`: аргументы программы | — | аргументы |
| [std/env.unio](../../stdlib/std/env.unio) | `std.env`: переменные, домашний каталог | — | обе функции |
| [std/files.unio](../../stdlib/std/files.unio) | `std.files`: чтение, каталоги, атомарная запись | `write_text`, `list_tree`, протокол фиксации `AtomicFile` | отдельные системные вызовы |

## Общие правила модулей

- Функция, которая может завершиться ошибкой, перечисляет конкретные типы ошибок (Б-2); `fails with Error` не используется.
- Операция, заблокированная песочницей, завершается объявленной ошибкой с `kind = PermissionDenied` (ПС-6). Функция, которая сообщает об отсутствии вместо ошибки (`files.exists`, `env.get`), сообщает о заблокированном объекте как об отсутствующем.
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
