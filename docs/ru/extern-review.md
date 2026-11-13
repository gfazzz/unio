# Проверка внешних функций стандартной библиотеки

Английская версия (основная): [docs/extern-review.md](../extern-review.md).

Требование Н-5 требует, чтобы весь код `external function` стандартной библиотеки проверили два человека и протестировали санитайзерами памяти. Этот документ — контрольный список такой проверки: он перечисляет каждую внешнюю функцию, которую связывает среда выполнения, с файлом, где она объявлена, функцией Rust, которая её реализует, полномочиями, которые она использует, и тем, что проверяют рецензенты. Саму проверку выполняют люди, и она записывается здесь; ни одна автоматическая проверка не считается одной из двух проверок. Состояние записи: **ожидает проверки людьми** — ни одна из двух проверок не проведена (строка Н-5 в [status.md](status.md)).

Часть с санитайзерами выполняется в CI: процесс [sanitizers.yml](../../.github/workflows/sanitizers.yml) запускает тесты крейта среды выполнения под AddressSanitizer и MemorySanitizer и каждый блок `test` исходников Unio через `unio`, собранный с AddressSanitizer, чтобы каждая внешняя функция вызывалась так, как её вызывают программы. Он использует ночной набор инструментов, закреплённый на одной дате, — единственная часть проекта, которая так делает: санитайзеры Rust существуют только за `-Zsanitizer`, а Miri не выполняет системные вызовы внешних функций.

Список сверяется со связываниями среды выполнения тестом `compiler/unio-runtime/tests/bindings.rs` (`the_review_checklist_lists_every_external`): внешняя функция, добавленная без строки, или строка без внешней функции проваливают тесты на обоих языках. Номера строк следуют порядку имён и могут измениться при добавлении функции; проверка ссылается на имя.

## Как проверять

Каждый рецензент читает объявление (его документирующий комментарий — спецификация, [runtime.md](runtime.md), раздел 7), связывание в `compiler/unio-runtime/src/externs/` и каждую функцию среды выполнения, которую оно вызывает, и проверяет:

1. **Нет паники.** Никакой вход — сколь угодно большой, отрицательный, пустой или не в UTF-8 — не приводит к панике, выходу индекса за границы, переполнению целого или делению на ноль; неверный аргумент — это сбой с сообщением, отказ системы — объявленная ошибка.
2. **Память.** Выделение, размер которого зависит от входа программы, учитывается в пределе кучи до того, как сделано, и использует выделение с возможным отказом, когда размер не ограничен (К-18, [runtime.md](runtime.md), 3.4); отказ в выделении в другом месте завершает программу с кодом 102.
3. **Полномочия.** Связывание не использует полномочий сверх столбца «Полномочия» и проверяет песочницу перед каждым системным вызовом, который их использует ([runtime.md](runtime.md), раздел 9).
4. **Код `unsafe`.** У каждого блока `unsafe`, достижимого из связывания, есть верный комментарий `SAFETY`: внешние объявления совпадают с заголовками системы на каждой платформе, указатели живут дольше вызова, буферы имеют указанную длину, `errno` и коды возврата читаются так, как их определяет система. Блоки `unsafe` в `externs/` на момент написания: `clock.rs` (`getentropy` и его аналог в Windows), `durability.rs` (`statfs`), `files.rs` (`MoveFileExW`, `renamex_np`, `close`, `fsync`) и `system.rs` (домашний каталог); связывания достигают также кода `unsafe` в `sandbox/` (`fdio.rs`, `beneath.rs`, `linux.rs`, `macos.rs`, `windows.rs`, `writer.rs`), в `value.rs` и `reserve.rs`.
5. **Особенности своего вида**, буква в столбце «Вид»:

- **A.** Преобразования между Int и usize (отрицательные, огромные); копирование при записи (Rc::make_mut) не меняет разделяемые значения; рост учитывается в куче; нет паники на любом индексе.
- **B.** Индексы — кодовые точки и проверяются (отрицательные, за концом); срезы только по границам символов; размеры из входа (repeat, replace, join) проверяются на переполнение и учитываются в куче до выделения.
- **C.** Переполнение, NaN, бесконечности и отрицательный ноль, как задано; результаты соответствуют IEEE 754 или описанному округлению; нет паники на любом входе.
- **D.** Нет нормализации, меняющей смысл пути; разделители платформы; нет паники на любом тексте.
- **E.** Раскрутка открывает резерв для release и закрывает его снова (К-17); после исчерпания не выполняется пользовательский код; тождество сравнивает ресурс, а не значение.
- **F.** Аргументы не в UTF-8; список учитывается в куче.
- **G.** Видны только переменные, разрешённые манифестом; значения не в UTF-8; нет выделений по размеру окружения вне учёта кучи.
- **H.** Проверка области песочницы до системного вызова; операция открывает проверенный путь, не следуя ссылкам (условие выпуска 5); владение дескриптором: нет использования после закрытия и двойного закрытия; каждая системная ошибка становится IoError, а не паникой; временные буферы учитываются в куче до выделения.
- **I.** Глубина вложенности ограничена; закрытые поля не заполняются (E0046); тождество типа по объявлению; нет паники на любом значении.
- **J.** Запускаются только перечисленные программы; дочерний процесс получает только разрешённые переменные; аргументы передаются без оболочки; тайм-аут останавливает всё дерево; вывод ограничен и учитывается в куче; дескрипторы и каналы закрываются на любом пути.
- **K.** Объявления FFI и пути ошибок (errno, короткое чтение, предел 256 байт у getentropy); нет переполнения при переводе наносекунд; сон ограничен и прерываем, как описано.
- **L.** Таблицы соответствуют заявленной версии Unicode; сегментация и нормализация не паникуют и не выходят за границы; размер результата ограничен и учтён.
- **M.** Текст записывается целиком или сообщается ошибка; закрытый или переполненный поток не вызывает паники; нет неограниченной буферизации.

Рецензент, нашедший дефект, открывает задачу и записывает её номер в строку вместо имени; строка подписывается, когда исправление влито и проверено снова.

## Внешние функции

Внешних функций: 118.

| # | Внешняя функция | Объявлена в | Связывание | Полномочия | Вид | Рецензент 1 | Рецензент 2 |
|---|---|---|---|---|---|---|---|
| 1 | `Bytes` | `stdlib/alloc/builtin/bytes.unio:7` | `externs/alloc.rs` `bytes_new` | нет | A | ожидает | ожидает |
| 2 | `Bytes.length` | `stdlib/alloc/builtin/bytes.unio:20` | `externs/alloc.rs` `bytes_length` | нет | A | ожидает | ожидает |
| 3 | `Bytes.slice` | `stdlib/alloc/builtin/bytes.unio:32` | `externs/alloc.rs` `bytes_slice` | нет | A | ожидает | ожидает |
| 4 | `Bytes.to_text` | `stdlib/alloc/builtin/bytes.unio:25` | `externs/alloc.rs` `bytes_to_text` | нет | B | ожидает | ожидает |
| 5 | `Float.is_finite` | `stdlib/core/builtin/float.unio:13` | `externs/core.rs` `is_finite` | нет | C | ожидает | ожидает |
| 6 | `Float.is_nan` | `stdlib/core/builtin/float.unio:8` | `externs/core.rs` `is_nan` | нет | C | ожидает | ожидает |
| 7 | `Float.to_bits` | `stdlib/core/builtin/float.unio:25` | `externs/math.rs` `to_bits` | нет | C | ожидает | ожидает |
| 8 | `Float.to_int` | `stdlib/core/builtin/float.unio:19` | `externs/math.rs` `to_int` | нет | C | ожидает | ожидает |
| 9 | `Float.to_text` | `stdlib/alloc/builtin/float.unio:6` | `externs/alloc.rs` `float_to_text_extern` | нет | C | ожидает | ожидает |
| 10 | `Int.checked_add` | `stdlib/core/builtin/int.unio:18` | `externs/core.rs` `checked_add` | нет | C | ожидает | ожидает |
| 11 | `Int.checked_multiply` | `stdlib/core/builtin/int.unio:28` | `externs/core.rs` `checked_multiply` | нет | C | ожидает | ожидает |
| 12 | `Int.checked_subtract` | `stdlib/core/builtin/int.unio:23` | `externs/core.rs` `checked_subtract` | нет | C | ожидает | ожидает |
| 13 | `Int.to_float` | `stdlib/core/builtin/int.unio:48` | `externs/core.rs` `int_to_float` | нет | C | ожидает | ожидает |
| 14 | `Int.to_text` | `stdlib/alloc/builtin/int.unio:5` | `externs/alloc.rs` `int_to_text` | нет | C | ожидает | ожидает |
| 15 | `List.append` | `stdlib/alloc/builtin/list.unio:21` | `externs/alloc.rs` `list_append` | нет | A | ожидает | ожидает |
| 16 | `List.get` | `stdlib/alloc/builtin/list.unio:14` | `externs/alloc.rs` `list_get` | нет | A | ожидает | ожидает |
| 17 | `List.length` | `stdlib/alloc/builtin/list.unio:9` | `externs/alloc.rs` `list_length` | нет | A | ожидает | ожидает |
| 18 | `List.remove_last` | `stdlib/alloc/builtin/list.unio:27` | `externs/alloc.rs` `list_remove_last` | нет | A | ожидает | ожидает |
| 19 | `Map.entries` | `stdlib/alloc/builtin/map.unio:33` | `externs/alloc.rs` `map_entries` | нет | A | ожидает | ожидает |
| 20 | `Map.get` | `stdlib/alloc/builtin/map.unio:14` | `externs/alloc.rs` `map_get` | нет | A | ожидает | ожидает |
| 21 | `Map.length` | `stdlib/alloc/builtin/map.unio:8` | `externs/alloc.rs` `map_length` | нет | A | ожидает | ожидает |
| 22 | `Map.put` | `stdlib/alloc/builtin/map.unio:22` | `externs/alloc.rs` `map_put` | нет | A | ожидает | ожидает |
| 23 | `Map.remove` | `stdlib/alloc/builtin/map.unio:28` | `externs/alloc.rs` `map_remove` | нет | A | ожидает | ожидает |
| 24 | `Path` | `stdlib/std/builtin/path.unio:10` | `externs/system.rs` `path_new` | нет | D | ожидает | ожидает |
| 25 | `Path.to_text` | `stdlib/std/builtin/path.unio:13` | `externs/system.rs` `path_to_text` | нет | D | ожидает | ожидает |
| 26 | `Set` | `stdlib/alloc/builtin/set.unio:10` | `externs/alloc.rs` `set_new` | нет | A | ожидает | ожидает |
| 27 | `Set.contains` | `stdlib/alloc/builtin/set.unio:21` | `externs/alloc.rs` `set_contains` | нет | A | ожидает | ожидает |
| 28 | `Set.insert` | `stdlib/alloc/builtin/set.unio:29` | `externs/alloc.rs` `set_insert` | нет | A | ожидает | ожидает |
| 29 | `Set.items` | `stdlib/alloc/builtin/set.unio:40` | `externs/alloc.rs` `set_items` | нет | A | ожидает | ожидает |
| 30 | `Set.length` | `stdlib/alloc/builtin/set.unio:16` | `externs/alloc.rs` `set_length` | нет | A | ожидает | ожидает |
| 31 | `Set.remove` | `stdlib/alloc/builtin/set.unio:35` | `externs/alloc.rs` `set_remove` | нет | A | ожидает | ожидает |
| 32 | `Text.byte_length` | `stdlib/alloc/builtin/text.unio:58` | `externs/alloc.rs` `text_byte_length` | нет | B | ожидает | ожидает |
| 33 | `Text.chars` | `stdlib/alloc/builtin/text.unio:63` | `externs/alloc.rs` `text_chars` | нет | B | ожидает | ожидает |
| 34 | `Text.code_points` | `stdlib/alloc/builtin/text.unio:68` | `externs/alloc.rs` `text_code_points` | нет | B | ожидает | ожидает |
| 35 | `Text.contains` | `stdlib/alloc/builtin/text.unio:128` | `externs/alloc.rs` `text_contains` | нет | B | ожидает | ожидает |
| 36 | `Text.count` | `stdlib/alloc/builtin/text.unio:143` | `externs/alloc.rs` `text_count` | нет | B | ожидает | ожидает |
| 37 | `Text.ends_with` | `stdlib/alloc/builtin/text.unio:138` | `externs/alloc.rs` `text_ends_with` | нет | B | ожидает | ожидает |
| 38 | `Text.index_of` | `stdlib/alloc/builtin/text.unio:88` | `externs/alloc.rs` `text_index_of` | нет | B | ожидает | ожидает |
| 39 | `Text.length` | `stdlib/alloc/builtin/text.unio:53` | `externs/alloc.rs` `text_length` | нет | B | ожидает | ожидает |
| 40 | `Text.lines` | `stdlib/alloc/builtin/text.unio:154` | `externs/alloc.rs` `text_lines` | нет | B | ожидает | ожидает |
| 41 | `Text.repeat` | `stdlib/alloc/builtin/text.unio:179` | `externs/alloc.rs` `text_repeat` | нет | B | ожидает | ожидает |
| 42 | `Text.replace` | `stdlib/alloc/builtin/text.unio:149` | `externs/alloc.rs` `text_replace` | нет | B | ожидает | ожидает |
| 43 | `Text.slice` | `stdlib/alloc/builtin/text.unio:74` | `externs/alloc.rs` `text_slice` | нет | B | ожидает | ожидает |
| 44 | `Text.split_whitespace` | `stdlib/alloc/builtin/text.unio:159` | `externs/alloc.rs` `text_split_whitespace` | нет | B | ожидает | ожидает |
| 45 | `Text.starts_with` | `stdlib/alloc/builtin/text.unio:133` | `externs/alloc.rs` `text_starts_with` | нет | B | ожидает | ожидает |
| 46 | `Text.to_float` | `stdlib/alloc/builtin/text.unio:103` | `externs/alloc.rs` `text_to_float_extern` | нет | B | ожидает | ожидает |
| 47 | `Text.to_lower` | `stdlib/alloc/builtin/text.unio:93` | `externs/alloc.rs` `text_to_lower` | нет | B | ожидает | ожидает |
| 48 | `Text.to_upper` | `stdlib/alloc/builtin/text.unio:98` | `externs/alloc.rs` `text_to_upper` | нет | B | ожидает | ожидает |
| 49 | `Text.trim` | `stdlib/alloc/builtin/text.unio:164` | `externs/alloc.rs` `text_trim` | нет | B | ожидает | ожидает |
| 50 | `Text.trim_end` | `stdlib/alloc/builtin/text.unio:174` | `externs/alloc.rs` `text_trim_end` | нет | B | ожидает | ожидает |
| 51 | `Text.trim_start` | `stdlib/alloc/builtin/text.unio:169` | `externs/alloc.rs` `text_trim_start` | нет | B | ожидает | ожидает |
| 52 | `assert_fail` | `stdlib/core/prelude.unio:63` | `externs/core.rs` `assert_fail` | нет (тесты и тождество) | E | ожидает | ожидает |
| 53 | `bytes_of` | `stdlib/alloc/builtin/bytes.unio:14` | `externs/alloc.rs` `bytes_of` | нет | A | ожидает | ожидает |
| 54 | `expect_fault` | `stdlib/core/prelude.unio:68` | `externs/core.rs` `expect_fault` | нет (тесты и тождество) | E | ожидает | ожидает |
| 55 | `is_same` | `stdlib/core/prelude.unio:75` | `externs/core.rs` `is_same` | нет (тесты и тождество) | E | ожидает | ожидает |
| 56 | `separator` | `stdlib/std/builtin/path.unio:141` | `externs/system.rs` `separator` | нет | D | ожидает | ожидает |
| 57 | `std.args.list` | `stdlib/std/args.unio:28` | `externs/system.rs` `args_list` | нет (аргументы) | F | ожидает | ожидает |
| 58 | `std.env.get` | `stdlib/std/env.unio:7` | `externs/system.rs` `env_get` | env | G | ожидает | ожидает |
| 59 | `std.env.home_dir` | `stdlib/std/env.unio:15` | `externs/system.rs` `env_home_dir` | env | G | ожидает | ожидает |
| 60 | `std.files.create_directory` | `stdlib/std/files.unio:457` | `externs/files.rs` `create_directory` | files.write | H | ожидает | ожидает |
| 61 | `std.files.exists` | `stdlib/std/files.unio:33` | `externs/files.rs` `exists` | files.read | H | ожидает | ожидает |
| 62 | `std.files.is_directory` | `stdlib/std/files.unio:38` | `externs/files.rs` `is_directory` | files.read | H | ожидает | ожидает |
| 63 | `std.files.list_directory` | `stdlib/std/files.unio:359` | `externs/files.rs` `list_directory` | files.read | H | ожидает | ожидает |
| 64 | `std.files.read_bytes` | `stdlib/std/files.unio:403` | `externs/files.rs` `read_bytes` | files.read | H | ожидает | ожидает |
| 65 | `std.files.read_text` | `stdlib/std/files.unio:48` | `externs/files.rs` `read_text` | files.read | H | ожидает | ожидает |
| 66 | `std.files.rename` | `stdlib/std/files.unio:445` | `externs/files.rs` `rename` | files.write | H | ожидает | ожидает |
| 67 | `std.files.resolve` | `stdlib/std/files.unio:350` | `externs/files.rs` `resolve` | files.read | H | ожидает | ожидает |
| 68 | `std.files.system_close` | `stdlib/std/files.unio:708` | `externs/files.rs` `system_close` | files.write | H | ожидает | ожидает |
| 69 | `std.files.system_open_read` | `stdlib/std/files.unio:205` | `externs/files.rs` `system_open_read` | files.read | H | ожидает | ожидает |
| 70 | `std.files.system_open_temporary` | `stdlib/std/files.unio:694` | `externs/files.rs` `system_open_temporary` | files.write | H | ожидает | ожидает |
| 71 | `std.files.system_read_chunk` | `stdlib/std/files.unio:211` | `externs/files.rs` `system_read_chunk` | files.read | H | ожидает | ожидает |
| 72 | `std.files.system_remove` | `stdlib/std/files.unio:720` | `externs/files.rs` `system_remove` | files.write | H | ожидает | ожидает |
| 73 | `std.files.system_rename` | `stdlib/std/files.unio:712` | `externs/files.rs` `system_rename` | files.write | H | ожидает | ожидает |
| 74 | `std.files.system_supports_durability` | `stdlib/std/files.unio:690` | `externs/files.rs` `system_supports_durability` | files.write | H | ожидает | ожидает |
| 75 | `std.files.system_sync_data` | `stdlib/std/files.unio:704` | `externs/files.rs` `system_sync_data` | files.write | H | ожидает | ожидает |
| 76 | `std.files.system_sync_directory` | `stdlib/std/files.unio:717` | `externs/files.rs` `system_sync_directory` | files.write | H | ожидает | ожидает |
| 77 | `std.files.system_write` | `stdlib/std/files.unio:697` | `externs/files.rs` `system_write` | files.write | H | ожидает | ожидает |
| 78 | `std.files.system_write_bytes` | `stdlib/std/files.unio:700` | `externs/files.rs` `system_write_bytes` | files.write | H | ожидает | ожидает |
| 79 | `std.json.from_value` | `stdlib/alloc/json.unio:102` | `externs/alloc.rs` `json_from_value` | нет | I | ожидает | ожидает |
| 80 | `std.json.to_value` | `stdlib/alloc/json.unio:100` | `externs/alloc.rs` `json_to_value` | нет | I | ожидает | ожидает |
| 81 | `std.math.acos` | `stdlib/core/math.unio:89` | `externs/math.rs` `acos` | нет | C | ожидает | ожидает |
| 82 | `std.math.asin` | `stdlib/core/math.unio:84` | `externs/math.rs` `asin` | нет | C | ожидает | ожидает |
| 83 | `std.math.atan` | `stdlib/core/math.unio:94` | `externs/math.rs` `atan` | нет | C | ожидает | ожидает |
| 84 | `std.math.atan2` | `stdlib/core/math.unio:100` | `externs/math.rs` `atan2` | нет | C | ожидает | ожидает |
| 85 | `std.math.cbrt` | `stdlib/core/math.unio:44` | `externs/math.rs` `cbrt` | нет | C | ожидает | ожидает |
| 86 | `std.math.ceil` | `stdlib/core/math.unio:120` | `externs/math.rs` `ceil` | нет | C | ожидает | ожидает |
| 87 | `std.math.cos` | `stdlib/core/math.unio:74` | `externs/math.rs` `cos` | нет | C | ожидает | ожидает |
| 88 | `std.math.exp` | `stdlib/core/math.unio:49` | `externs/math.rs` `exp` | нет | C | ожидает | ожидает |
| 89 | `std.math.floor` | `stdlib/core/math.unio:115` | `externs/math.rs` `floor` | нет | C | ожидает | ожидает |
| 90 | `std.math.from_bits` | `stdlib/core/math.unio:136` | `externs/math.rs` `from_bits` | нет | C | ожидает | ожидает |
| 91 | `std.math.hypot` | `stdlib/core/math.unio:110` | `externs/math.rs` `hypot` | нет | C | ожидает | ожидает |
| 92 | `std.math.ln` | `stdlib/core/math.unio:54` | `externs/math.rs` `ln` | нет | C | ожидает | ожидает |
| 93 | `std.math.log10` | `stdlib/core/math.unio:64` | `externs/math.rs` `log10` | нет | C | ожидает | ожидает |
| 94 | `std.math.log2` | `stdlib/core/math.unio:59` | `externs/math.rs` `log2` | нет | C | ожидает | ожидает |
| 95 | `std.math.power` | `stdlib/core/math.unio:105` | `externs/math.rs` `power` | нет | C | ожидает | ожидает |
| 96 | `std.math.round` | `stdlib/core/math.unio:125` | `externs/math.rs` `round` | нет | C | ожидает | ожидает |
| 97 | `std.math.sin` | `stdlib/core/math.unio:69` | `externs/math.rs` `sin` | нет | C | ожидает | ожидает |
| 98 | `std.math.sqrt` | `stdlib/core/math.unio:39` | `externs/math.rs` `sqrt` | нет | C | ожидает | ожидает |
| 99 | `std.math.tan` | `stdlib/core/math.unio:79` | `externs/math.rs` `tan` | нет | C | ожидает | ожидает |
| 100 | `std.math.truncate` | `stdlib/core/math.unio:130` | `externs/math.rs` `truncate` | нет | C | ожидает | ожидает |
| 101 | `std.process.run` | `stdlib/std/process.unio:48` | `externs/process.rs` `run` | process | J | ожидает | ожидает |
| 102 | `std.random.entropy` | `stdlib/std/random.unio:88` | `externs/clock.rs` `entropy` | случайность системы | K | ожидает | ожидает |
| 103 | `std.random.secure_bytes` | `stdlib/std/random.unio:101` | `externs/clock.rs` `secure_bytes` | случайность системы | K | ожидает | ожидает |
| 104 | `std.random.secure_int` | `stdlib/std/random.unio:120` | `externs/clock.rs` `secure_int` | случайность системы | K | ожидает | ожидает |
| 105 | `std.text.decode_utf8_step` | `stdlib/alloc/text.unio:105` | `externs/alloc.rs` `decode_utf8_step` | нет | B | ожидает | ожидает |
| 106 | `std.text.from_code_point` | `stdlib/alloc/text.unio:39` | `externs/alloc.rs` `from_code_point` | нет | B | ожидает | ожидает |
| 107 | `std.text.graphemes` | `stdlib/alloc/text.unio:64` | `externs/alloc.rs` `text_graphemes` | нет | L | ожидает | ожидает |
| 108 | `std.text.join` | `stdlib/alloc/text.unio:20` | `externs/alloc.rs` `text_join` | нет | B | ожидает | ожидает |
| 109 | `std.text.split` | `stdlib/alloc/text.unio:26` | `externs/alloc.rs` `text_split` | нет | B | ожидает | ожидает |
| 110 | `std.text.to_nfc` | `stdlib/alloc/text.unio:48` | `externs/alloc.rs` `text_to_nfc` | нет | L | ожидает | ожидает |
| 111 | `std.text.to_nfd` | `stdlib/alloc/text.unio:56` | `externs/alloc.rs` `text_to_nfd` | нет | L | ожидает | ожидает |
| 112 | `std.text.unicode_version` | `stdlib/alloc/text.unio:69` | `externs/alloc.rs` `unicode_version` | нет | L | ожидает | ожидает |
| 113 | `std.time.monotonic_nanoseconds` | `stdlib/std/time.unio:510` | `externs/clock.rs` `monotonic_nanoseconds` | часы | K | ожидает | ожидает |
| 114 | `std.time.sleep_nanoseconds` | `stdlib/std/time.unio:514` | `externs/clock.rs` `sleep_nanoseconds` | часы | K | ожидает | ожидает |
| 115 | `std.time.wall_nanoseconds` | `stdlib/std/time.unio:512` | `externs/clock.rs` `wall_nanoseconds` | часы | K | ожидает | ожидает |
| 116 | `total_order_key` | `stdlib/core/builtin/float.unio:56` | `externs/core.rs` `total_order_key` | нет | C | ожидает | ожидает |
| 117 | `write_error` | `stdlib/std/prelude.unio:18` | `externs/system.rs` `write_error` | стандартные потоки | M | ожидает | ожидает |
| 118 | `write_output` | `stdlib/std/prelude.unio:16` | `externs/system.rs` `write_output` | стандартные потоки | M | ожидает | ожидает |
