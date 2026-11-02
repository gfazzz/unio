# Стандартная библиотека: 0.1-draft

Версия 0.1-draft. Английская версия (основная): [../stdlib.md](../stdlib.md). Основание: раздел 5 ТЗ, разделы 2–4 концепции.

Стандартная библиотека написана на Unio в [stdlib/std/](../../stdlib/std). Исходники — нормативный API: сигнатуры, документирующие комментарии и реализации. Всё, что язык может выразить, реализовано на Unio; только примитивы операционной системы и среды выполнения объявлены как `external function`, и только в файлах с пометкой `unsafe module` (Б-7, Б-10). Этот документ — обзор; справочник по исходникам будет строить `unio doc` (Д-3).

Библиотека содержит то, чем пользуются эталонные программы и инструменты проекта. `spec/check_api.py` проверяет, что `docs/examples/`, `examples/`, `tools/` и сама библиотека не вызывают ничего, что в ней не объявлено. Остальные модули требования Б-1 (`std.collections`, `std.math`, `std.decimal`, `std.path`, `std.process`, `std.time`, `std.random`, `std.log`, `std.test`) добавляются на этапе 5 в том же виде.

## Файлы

| Файл | Содержание | На Unio | Внешние примитивы |
|---|---|---|---|
| [prelude.unio](../../stdlib/std/prelude.unio) | `Display`, `Error`, `Equal`, `Order`, `Hash`, интерфейсы операторов, `Resource`, `Option`, `Result`, `print`, `print_error`, `range`, `min`, `max` | интерфейсы, перечисления, `print`, `range`, `min`, `max` | стандартный вывод, `assert_fail`, `expect_fault`, `is_same` |
| [builtin/text.unio](../../stdlib/std/builtin/text.unio) | Методы `Text` | поиск, разбиение, обрезка, `char_at`, `to_int`, `replace`, `lines` | длина, символы, срезы, `index_of`, регистр, `to_float` |
| [builtin/list.unio](../../stdlib/std/builtin/list.unio) | Методы `List[T]` | `find`, `map`, `filter`, `enumerate`, `contains`, `max`, `sorted`, `sorted_by`, `sorted_with` (сортировка слиянием) | длина, `get`, `append`, `remove_last` |
| [builtin/option.unio](../../stdlib/std/builtin/option.unio) | Методы `Option[T]` | `is_some`, `is_none` | — |
| [builtin/set.unio](../../stdlib/std/builtin/set.unio) | `Set[T]` и `List.to_set` | `union`, `intersection`, `difference`, `to_set` | хранение элементов |
| [builtin/map.unio](../../stdlib/std/builtin/map.unio) | Методы `Map[K, V]` | `contains`, `keys`, `values` | хранение записей |
| [builtin/path.unio](../../stdlib/std/builtin/path.unio) | `Path` | компоненты, `name`, `extension`, `parent`, `join`, `normalize` | создание, текстовая форма, разделитель платформы |
| [builtin/int.unio](../../stdlib/std/builtin/int.unio), [float.unio](../../stdlib/std/builtin/float.unio), [bool.unio](../../stdlib/std/builtin/bool.unio) | Числа и `Bool` | `checked_divide`, `Bool.to_text` | арифметика с проверкой, преобразования |
| [text.unio](../../stdlib/std/text.unio) | `std.text`: `join`, `split` | обе | `from_code_point` |
| [args.unio](../../stdlib/std/args.unio) | `std.args`: аргументы программы | — | аргументы |
| [env.unio](../../stdlib/std/env.unio) | `std.env`: переменные, домашний каталог | — | обе функции |
| [files.unio](../../stdlib/std/files.unio) | `std.files`: чтение, каталоги, атомарная запись | `write_text`, `list_tree`, логика `AtomicFile` | открытие, чтение, список, запись, переименование |
| [json.unio](../../stdlib/std/json.unio) | `std.json`: `Value`, `parse`, `encode`, `decode` | разбор и запись | преобразования между типом и `Value`, которые порождает компилятор |

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
