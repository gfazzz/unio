# Стандартная библиотека: 0.1-draft

Версия 0.1-draft. Английская версия (основная): [../stdlib.md](../stdlib.md). Основание: раздел 5 ТЗ, разделы 2–4 концепции.

Стандартная библиотека написана на Unio в [stdlib/](../../stdlib) и разделена на три слоя. Исходники — нормативный API: сигнатуры, документирующие комментарии и реализации. Всё, что язык может выразить, реализовано на Unio; только примитивы операционной системы и среды выполнения объявлены как `external function`, и только в файлах с пометкой `unsafe module` (Б-7, Б-10). Внешние функции реализует среда выполнения в [compiler/unio-runtime](../../compiler/unio-runtime) (115 функций, [runtime.md](runtime.md), раздел 7), и тест там падает, если объявление и привязка расходятся. Этот документ — обзор, написанный вручную; `unio doc`, который должен строить справочник по исходникам (Д-3), пока не существует. Что реализовано и проверено, по требованиям, — в [status.md](status.md).

Библиотека содержит то, чем пользуются эталонные программы и инструменты проекта. `tools/check_api.unio` проверяет в CI, что `docs/examples/`, `examples/`, `tools/` и сама библиотека не вызывают ничего, что в ней не объявлено (его эталон на Python, `spec/check_api.py`, даёт тот же результат), а `unio test` выполняет блоки `test` каждого модуля в CI на Linux, macOS и Windows. Все пятнадцать модулей требования Б-1 существуют.

У каждого модуля, публичной функции и публичной константы есть документирующий комментарий с описанием и примером (Б-3). Пример — блок строк с отступом в четыре пробела под строкой `## Example:`; он записан так, как выполняется внутри своего модуля, и `unio test` запускает его как тест этого модуля (И-13), поэтому примеры проверяются наравне с блоками `test`. `compiler/unio-runtime/tests/documentation.rs` падает, если у публичной функции нет описания или примера. У библиотеки нет манифеста, поэтому её тесты и примеры выполняются без полномочий: примеры `std.files` и `std.process` показывают пути отказа, одинаковые с полномочиями и без них.

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
| [core/builtin/int.unio](../../stdlib/core/builtin/int.unio), [float.unio](../../stdlib/core/builtin/float.unio), [bool.unio](../../stdlib/core/builtin/bool.unio), [option.unio](../../stdlib/core/builtin/option.unio) | Числа, `Bool`, `Option` | `checked_divide`, `abs`, `checked_abs`, `sign`, `Float.abs`, `Bool.to_text`, `is_some`, `is_none`, `OrderedFloat`, `Float.total_order` | арифметика с проверкой, `is_nan`, `is_finite`, `Float.to_int`, `Float.to_bits`, канонический ключ Float |
| [core/math.unio](../../stdlib/core/math.unio) | `std.math`: `PI`, `TAU`, `E`, `INFINITY`, `NAN`, элементарные функции и округление `Float`, `clamp`, `gcd`, `lcm`, `checked_power`, `integer_sqrt`, `floor_divide`, `modulo` | целочисленные функции и `clamp` | функции `Float` (IEEE 754, `libm` платформы), `from_bits` |
| [alloc/prelude.unio](../../stdlib/alloc/prelude.unio) | `Range.to_list` | `to_list` | — |
| [alloc/builtin/text.unio](../../stdlib/alloc/builtin/text.unio) | Методы `Text` | поиск, разбиение, обрезка, `char_at`, `to_int`, `replace`, `lines` | длина, символы, срезы, `index_of`, регистр, `to_float` |
| [alloc/builtin/list.unio](../../stdlib/alloc/builtin/list.unio) | Методы `List[T]` | `find`, `map`, `filter`, `enumerate`, `contains`, `max`, `sorted`, `sorted_by`, `fold`, `partition`, `group_by`, `zip`, `chunks`, `windows`, `try_map/filter/fold`, `iterator`, `sorted_with` (сортировка слиянием) | длина, `get`, `append`, `remove_last` |
| [alloc/builtin/map.unio](../../stdlib/alloc/builtin/map.unio), [set.unio](../../stdlib/alloc/builtin/set.unio) | `Map[K, V]`, `Set[T]`, `List.to_set` | `contains`, `keys`, `values`, `union`, `intersection`, `difference` | хранение записей |
| [alloc/builtin/int.unio](../../stdlib/alloc/builtin/int.unio), [float.unio](../../stdlib/alloc/builtin/float.unio) | Текстовая форма чисел | — | `to_text` |
| [alloc/builtin/bytes.unio](../../stdlib/alloc/builtin/bytes.unio) | `Bytes`, `bytes_of` | `is_empty`, `checked_slice` | создание (`Bytes()`, `bytes_of` из значений 0..255), `length`, `slice`, `to_text` |
| [alloc/text.unio](../../stdlib/alloc/text.unio) | `std.text`: `join`, `split`, `from_code_point`, `to_nfc`, `to_nfd`, `graphemes`, `unicode_version`; потоковый декодер UTF-8 `Utf8Decoder` (`utf8_decoder`, `decode`, `finish`, `pending_bytes`) с `Utf8Error` и `Utf8Problem` (Б-18) | состояние декодера, его смещения и ошибки | `join`, `split`, `from_code_point`, нормализация и графемы на таблицах базы символов Unicode 15.0.0 в среде выполнения; один шаг проверки UTF-8 (`decode_utf8_step`) |
| [alloc/collections.unio](../../stdlib/alloc/collections.unio) | `std.collections`: `Deque`, `PriorityQueue`, `counts`, `most_common` | все | — |
| [alloc/decimal.unio](../../stdlib/alloc/decimal.unio) | `std.decimal`: `Decimal`, `Rounding`, `parse`, `from_int`, `from_float` (Б-1a) | все: целые любого размера по основанию 10^9 | — |
| [alloc/json.unio](../../stdlib/alloc/json.unio) | `std.json`: `Value`, `parse`, `encode`, `encode_checked`, `decode` | разбор и запись | преобразования между типом и `Value`, которые порождает компилятор |
| [alloc/errors.unio](../../stdlib/alloc/errors.unio) | `std.errors`: `ContextError`, `context` | структурированная причина | — |
| [alloc/test.unio](../../stdlib/alloc/test.unio) | `std.test`: `check_property`, `Counterexample` | генерация, уменьшение, данные воспроизведения | — |
| [std/prelude.unio](../../stdlib/std/prelude.unio) | `print`, `print_error` | обе | стандартный вывод и поток ошибок |
| [std/builtin/path.unio](../../stdlib/std/builtin/path.unio) | `Path` | компоненты, `name`, `extension`, `parent`, `join`, `normalize` | создание, текстовая форма, разделитель платформы |
| [std/args.unio](../../stdlib/std/args.unio) | `std.args`: аргументы программы и разборщик позиционных аргументов, флагов и параметров со значениями по умолчанию и автоматической справкой (Б-4) | разборщик и `help` | аргументы |
| [std/path.unio](../../stdlib/std/path.unio) | `std.path`: `stem`, `with_extension`, `with_name`, `starts_with`, `relative_to`, `join_all`, `is_relative` | все, лексически | — |
| [std/time.unio](../../stdlib/std/time.unio) | `std.time`: `Duration`, `Instant`, `Timestamp`, `DateTime`, `now`, `monotonic`, `sleep`, текст RFC 3339, `Clock`, `SystemClock`, `ManualClock` (`manual_clock`) | длительности, календарь, RFC 3339 | монотонные и настенные часы, сон (эффект `clock`) |
| [std/random.unio](../../stdlib/std/random.unio) | `std.random`: `Generator` (MRG32k3a, не для криптографии), `seeded`, `from_entropy`; `secure_bytes`, `secure_int` и `RandomError` — единственный API, пригодный для ключей и секретов | генератор, ограниченные целые, числа с плавающей точкой, `shuffled`, `choose` | энтропия системы (эффект `random`); отсутствие источника — `RandomError`, никогда не более слабый генератор |
| [std/log.unio](../../stdlib/std/log.unio) | `std.log`: `Level`, `Logger`, `debug`, `info`, `warning`, `error`, `format_record` | все, в стандартный поток ошибок | — |
| [std/env.unio](../../stdlib/std/env.unio) | `std.env`: переменные, домашний каталог | — | обе функции |
| [std/files.unio](../../stdlib/std/files.unio) | `std.files`: ограниченные потоки байтов и текста, разрешение путей, каталоги, атомарная запись, копирование, переименование | `open_reader`, `FileReader.read_bytes/read_text/release`, `write_text`, `write_bytes`, `copy_file`, `create_directories`, `list_tree`, протокол фиксации `AtomicFile` | отдельные системные вызовы, `rename` без замены |
| [std/process.unio](../../stdlib/std/process.unio) | `std.process`: запуск программ без оболочки, с ограничением времени | — | `run` |

## Общие правила модулей

- Функция, которая может завершиться ошибкой, перечисляет конкретные типы ошибок (Б-2); `fails with Error` не используется, что `tools/check_library.unio` проверяет в CI.
- Операция, заблокированная песочницей, завершается объявленной ошибкой с `kind = PermissionDenied` (SB-6). Функция, которая сообщает об отсутствии вместо ошибки (`files.exists`, `env.get`), сообщает о заблокированном объекте как об отсутствующем.
- У каждой аварийной операции есть безопасная форма (Я-18): `items[i]` и `items.get(i)`, `a / b` и `a.checked_divide(b)`; опись их всех — раздел «Аварийные операции и их безопасные формы» ниже.
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

## Аварийные операции и их безопасные формы (Я-18)

У каждой операции библиотеки, которая может завершиться аварией, есть безопасная форма, возвращающая `Option` или ошибку, чтобы программа, которой нельзя останавливаться, могла проверить заранее. Исчерпание памяти или стека (код 102) — не авария (Я-17a), и безопасной формы у него нет. Таблица — это опись; `tools/check_library.unio` читает её в CI и не проходит, когда у аварийной операции нет строки, когда безопасная форма — не публичная функция, возвращающая `Option` или ошибку, или когда эта таблица и английская называют разные операции. Аварийные операции он находит как операторы ниже и публичные функции, в документирующем комментарии которых сказано, что они завершаются аварией, которые вызывают `assert_fail` или вызывают аварийную частную функцию своего файла; функция, комментарий которой называет её «the safe form of» другой, — безопасная форма. Авария, которую функция наследует от вызова другой публичной функции, находится, только если о ней говорит её комментарий, как и делают комментарии библиотеки. Тот же инструмент проверяет Б-2: в `stdlib/` нет `fails with Error`.

| Операция | Авария, когда | Безопасная форма |
|---|---|---|
| `a + b` | сумма двух `Int` не помещается | `Int.checked_add` |
| `a - b` | разность не помещается | `Int.checked_subtract` |
| `a * b` | произведение не помещается | `Int.checked_multiply` |
| `a / b` | `b` равно 0, или `MINIMUM_INT / -1` | `Int.checked_divide` |
| `a % b` | `b` равно 0 | `Int.checked_remainder` |
| `a ** b` | степень не помещается, или `b` отрицательно | `std.math.checked_power` |
| `-a` | `a` равно `MINIMUM_INT` | `Int.checked_subtract`, как `0.checked_subtract(a)` |
| `items[index]` | `index` вне `0..length()` | `List.get` |
| `map[key]` | ключа нет | `Map.get` |
| `Int.abs` | значение равно `MINIMUM_INT` | `Int.checked_abs` |
| `List.slice` | границы вне `0..length()` или переставлены | `List.checked_slice` |
| `Text.slice` | границы вне `0..length()` или переставлены | `Text.checked_slice` |
| `Bytes.slice` | границы вне `0..length()` или переставлены | `Bytes.checked_slice` |
| `Text.char_at` | позиция вне `0..length()` | `Text.checked_char_at` |
| `Text.repeat` | число повторений отрицательно | `Text.checked_repeat` |
| `std.text.split` | разделитель пуст | `std.text.checked_split` |
| `Decimal.divide` (`a / b` для `Decimal`) | делитель равен нулю, или у частного нет конечной записи | `Decimal.checked_divide` |
| `Decimal.round` | `places` отрицательно | `Decimal.checked_round` |
| `Decimal.divide_rounded` | делитель равен нулю, или `places` отрицательно | `Decimal.checked_divide_rounded` |
| `Decimal.to_fixed` | `places` отрицательно | `Decimal.checked_to_fixed` |
| `std.math.clamp` | `low` больше `high` | `std.math.checked_clamp` |
| `std.math.gcd` | результат равен 2^63 | `std.math.checked_gcd` |
| `std.math.integer_sqrt` | аргумент отрицателен | `std.math.checked_integer_sqrt` |
| `std.math.floor_divide` | делитель равен 0, или `MINIMUM_INT` делится на -1 | `std.math.checked_floor_divide` |
| `std.math.modulo` | делитель равен 0 | `std.math.checked_modulo` |
| `std.args.parser` | объявление не может работать (повторённое имя, обязательный позиционный после необязательного, ...) | `std.args.checked_parser` |
| `Arguments.flag` | имя не объявлено как флаг | `Arguments.checked_flag` |
| `Arguments.get` | имя не объявлено или это флаг | `Arguments.checked_get` |
| `Arguments.value` | как у `get`, или у параметра нет значения | `Arguments.checked_get` |
| `Arguments.values` | имя не объявлено | `Arguments.checked_values` |
| `Arguments.get_int` | имя не объявлено или это флаг | `Arguments.checked_get` с `Text.to_int` |
| `std.random.from_entropy` | у системы нет источника случайности | `std.random.checked_from_entropy` |
| `std.random.secure_bytes` | число меньше 0 или больше 1048576 | `std.random.checked_secure_bytes` |
| `std.random.secure_int` | `low >= high` | `std.random.checked_secure_int` |
| `Generator.next_int` | `low >= high` | `Generator.checked_next_int` |
| `std.time.microseconds` | длительность не помещается в `Int` наносекунд | `Duration.checked_times`, как `microseconds(1).checked_times(n)` |
| `std.time.milliseconds` | длительность не помещается | `Duration.checked_times` |
| `std.time.seconds` | длительность не помещается | `Duration.checked_times` |
| `std.time.minutes` | длительность не помещается | `Duration.checked_times` |
| `std.time.hours` | длительность не помещается | `Duration.checked_times` |
| `std.time.from_unix_seconds` | момент вне 1677–2262 годов | `Duration.checked_times` с `std.time.from_unix_nanoseconds` |
| `Duration.add` (`a + b` для `Duration`) | сумма не помещается | `Duration.checked_add` |
| `Duration.subtract` (`a - b` для `Duration`) | разность не помещается | `Duration.checked_subtract` |
| `Instant.since` | разность не помещается | `Instant.checked_since` |
| `Instant.plus` | показание не помещается | `Instant.checked_plus` |
| `Timestamp.since` | разность не помещается | `Timestamp.checked_since` |
| `Timestamp.plus` | момент вне 1677–2262 годов | `Timestamp.checked_plus` |
| `ManualClock.advance` | длительность отрицательна, или время не помещается | `ManualClock.checked_advance` |
| `assert_fail` | всегда: авария — её назначение | нет |
| `expect_fault` | действие не завершается аварией: тест не проходит | нет: она есть только в блоках `test` |

## Пути на каждой платформе

`Path` и `std.path` работают лексически (Б-19): они не читают файловую систему и ничего не доказывают о доступе. Их контракт платформы:

| | Linux и macOS | Windows |
|---|---|---|
| Разделители | `/`; `\` — обычный символ имени | `/` и `\`; `components()`, а также `parent`, `normalize` пишут `/` |
| `join` (и `with_name`, `with_extension`, `join_all`) | вставляет `/` | вставляет `\` |
| Абсолютный (`is_absolute`, не `is_relative`) | начинается с `/` (якорь `/` или `//`) | диск с корнем, `C:\a` или `C:/a` (якорь `C:/`); UNC-ресурс, `\\server\share\a` (якорь `//server/share/`) |
| Относительный от корня | — | `\a` или `/a` (якорь `/`): относительно корня текущего диска, не абсолютный |
| Относительный к диску | — | `C:a` (якорь `C:`): относительно текущего каталога диска C, не абсолютный |
| В текущем каталоге: `starts_with(p, Path("."))`, `relative_to(p, Path("."))` | путь без якоря | путь без якоря: не `\a`, `/a`, `C:a` |

Якорь — первый компонент пути, он сравнивается целиком, поэтому `starts_with(Path("C:a"), Path("C:"))` выполняется в Windows, а `starts_with(Path("C:\\a"), Path("C:"))` — нет. Тесты сравнивают пути, построенные `join`, с `Path(...).join(...)` или по `components()`, а не с текстом с одним из разделителей; тесты [stdlib/std/path.unio](../../stdlib/std/path.unio) проверяют каждую форму на той платформе, где запущены.

## Контракты и примеры RFC 0001

Изменения относительно исходного 0.1-draft: [контракты](language-contracts.md) уточняют поведение и слои. Новые реализации и `test`-блоки написаны на Unio, и `unio test` их выполняет. `FileReader`, `files.resolve`, ключ представления Float (`total_order_key`), преобразования JSON, порождаемые компилятором, и `Error.diagnostic_code` реализованы интерпретатором, нативным кодом и средой выполнения. Преобразование JSON типа с приватным полем — E0046, а декодирование такой структуры во время выполнения завершается `DecodeError` и не заполняет ни одного поля (Б-17). Известные пробелы ([status.md](status.md)): `std.test` содержит ядро property-тестов, но встраивание в запуск тестов и JSON-воспроизведение (И-12) не реализованы.

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
| Пути | `Path("link/../file").normalize()` сохраняет `..`; `try files.resolve(path)` обращается к ОС; `components()` сохраняет диск/UNC как единый якорь; `path.relative_to(file, base)` работает лексически |
| Потоки текста | `reader.read_text(max_bytes = 65536)` возвращает some(текст) или none в конце; символ, разрезанный между чтениями, сохраняется до завершения, неверный UTF-8 даёт `IoError` вида `InvalidData` со смещением в байтах после текста перед ним, а файл, кончающийся внутри символа, даёт ту же ошибку (Б-16, Б-18); `text.utf8_decoder()` делает то же для любых порций `Bytes`, храня между ними не более 3 байт |
| Unicode | `text.to_nfc(a) == text.to_nfc(b)` сравнивает канонически эквивалентные записи; `text.graphemes(s)` сохраняет семейный эмодзи или флаг целым; ничто не нормализуется неявно |
| Decimal | `decimal.parse("0.1")` плюс `decimal.parse("0.2")` — ровно 0.3; `a / b` точно или авария, `a.divide_rounded(b, 2, Rounding.HalfEven)` называет округление; `decimal.from_float(x, 2, Rounding.HalfUp)` — единственный путь из Float |
| Время | `time.monotonic().since(start)` измеряет; `time.now()` — текст RFC 3339 в UTC; функция принимает `clock as time.Clock`, а тест передаёт `time.manual_clock(...)` |
| Случайность | `var generator = random.seeded(42)` повторяется на всех платформах и не имеет эффекта; `random.from_entropy()` различается от запуска к запуску; ни то, ни другое не для криптографии: ключи берутся из `try random.secure_bytes(32)`, который завершается ошибкой `RandomError`, а не откатывается; никакой переключатель не даёт источнику системы зерно в тестах, тест передаёт свой `Generator` |
| Аргументы | `args.parse(args.parser("copy", "...", [args.flag(...), args.option(..., default = some("644")), args.positional(...)]), args.list())`; `--help` даёт `ArgsError` вида `HelpRequested` со сгенерированным текстом |

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
