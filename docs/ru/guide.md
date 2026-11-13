# Руководство по Unio

Версия 0.1-draft. Основная версия на английском: [../guide.md](../guide.md). Требование Д-2: от установки до публикации пакета на примере `todo` и глава о переходе от классов к Unio.

Руководство строит одну программу, `todo`, — консольный список задач — в четыре шага. Каждый шаг — файл в [examples/](examples): [todo_1_tasks.unio](examples/todo_1_tasks.unio), [todo_2_commands.unio](examples/todo_2_commands.unio), [todo_3_storage.unio](examples/todo_3_storage.unio) и полная [todo.unio](examples/todo.unio). Глава о классах — [classes.unio](examples/classes.unio). Каждый из них проверяют `unio check` и `unio fmt --check`, а его тесты выполняет `unio test` в CI на Linux, macOS и Windows, поэтому показанный здесь код работает так, как написан. Код в руководстве скопирован из этих файлов.

Определение языка — [справочник языка](reference.md); библиотека описана в [справочнике стандартной библиотеки](stdlib-reference.md).

## 1. Установка

Выпуск 0.1 ещё не опубликован: `unio` собирается из этого репозитория. Для сборки нужен набор инструментов Rust, закреплённый в `compiler/rust-toolchain.toml` (rustup установит его при первой сборке), и системный компоновщик, которым пользуется `unio build`: компилятор C `cc` в Linux, инструменты командной строки Xcode в macOS или Visual Studio Build Tools с нагрузкой C++ в Windows ([compiler/README.md](../../compiler/README.md)).

```sh
git clone <URL репозитория Unio> unio && cd unio
sh .github/scripts/release_build.sh "$HOME/.local/unio"
export PATH="$HOME/.local/unio/bin:$PATH"
unio --help
```

Сценарий собирает выпуск закреплёнными инструментами по файлу блокировки и устанавливает такую раскладку:

```text
PREFIX/bin/unio                  компилятор со встроенной стандартной библиотекой
PREFIX/lib/unio/libunio.a        среда выполнения для `unio build` (unio.lib в Windows)
PREFIX/share/doc/unio/           LICENSE-MIT, LICENSE-APACHE, THIRD-PARTY-NOTICES
```

Больше `unio` при работе ничего не нужно: стандартная библиотека встроена в него, а `unio build` находит среду выполнения в `lib/unio/` рядом с `bin/`. Две сборки в двух каталогах дают одинаковые байты. Для редактора установите расширение из [editors/](../../editors/README.md): подсветку и языковой сервер `unio lsp` с диагностиками, переходом к определению, типами, дополнением, форматированием и переименованием.

## 2. Инструменты

Всё делает одна команда; каждую подкоманду описывает `unio --help`.

| Команда | Что делает |
|---|---|
| `unio run FILE [-- ARGS]` | проверяет программу и выполняет её в интерпретаторе в песочнице её `unio.toml` |
| `unio check FILE...` | все статические проверки без запуска; `--diagnostics=json` для инструментов и агентов |
| `unio test FILE...` | выполняет блоки `test`, каждый отдельно; `--json` для машиночитаемого отчёта |
| `unio fmt FILE...` | переписывает файлы в едином стиле; `--check` только сообщает |
| `unio build FILE -o OUT` | исполняемый файл для этой машины |
| `unio outline FILE`, `unio symbol MODULE.NAME` | что объявляет файл; одно объявление с документацией, ошибками и эффектами, а для типа — реализуемые им интерфейсы с методом, который он использует для каждого |
| `unio explain CODE` | объяснение кода диагностики с примером и исправлением |

Программа — файл с `public function main()`. Её каталог или ближайший каталог выше с `unio.toml` — её пакет; манифест называет пакет и выдаёт ему разрешения. Без разрешений программа может читать свои аргументы и стандартный ввод и писать в стандартный вывод и стандартный поток ошибок, и больше ничего.

## 3. Шаг 1: задачи как значения

[todo_1_tasks.unio](examples/todo_1_tasks.unio) хранит задачи в списке в памяти. Задача — `struct`: запись с именованными типизированными полями. `public` делает поле видимым для других модулей, `var` разрешает его менять; всё остальное закрыто и неизменно.

```unio
struct Task:
    public id as Int
    public title as Text
    public var done as Bool = false
```

Функции указывают типы параметров и результата. Внутри функции `let` и `var` выводят тип локального имени. Лямбда, записанная там, где ожидается тип функции, берёт типы оттуда: `map` ожидает функцию от `Task`, поэтому `function (task): task.id` их не требует. `or` даёт значение по умолчанию для `Option`: `max()` пустого списка — `none`.

```unio
## Номер новой задачи: на единицу больше наибольшего номера в списке.
function next_id(tasks as List[Task]) returns Int:
    return (tasks.map(function (task): task.id).max() or 0) + 1

## Добавляет задачу в список вызывающего; var делает изменение видимым в месте вызова.
function add_task(var tasks as List[Task], title as Text):
    tasks.append(Task(id = next_id(tasks), title = title))
```

Значения — это значения: `Task`, взятая из списка, — копия, и её изменение список не меняет. Функция меняет данные вызывающего только через параметр `var`, а вызывающий передаёт переменную `var`, поэтому каждое изменение видно в месте вызова. Элемент меняется через сам список: `tasks[0].done = true`. Текст собирается интерполяцией, `"[{mark}] {task.id}. {task.title}"`; `+` между текстом и числом нет.

Тесты лежат рядом с кодом. `unio test` выполняет каждый блок `test` в свежем состоянии:

```unio
test "задача — значение: копия не меняет список":
    var tasks as List[Task] = []
    add_task(tasks, "a")
    var copy = tasks[0]
    copy.done = true
    assert not tasks[0].done
    tasks[0].done = true
    assert render(tasks[0]) == "[x] 1. a"
```

```sh
cd docs/ru/examples
unio test todo_1_tasks.unio
unio run todo_1_tasks.unio
```

Когда что-то не так, `unio check` говорит, что, где и как исправить, с кодом, который описывает `unio explain`. Если написать `copy.done = true` при `let copy` вместо `var copy`, получится E0078 — изменение через неизменяемое место.

## 4. Шаг 2: команды и ошибки

[todo_2_commands.unio](examples/todo_2_commands.unio) читает команду из аргументов. Команды — `enum`, закрытый набор вариантов, у которых могут быть поля, а неверная команда — `error`, отдельный тип:

```unio
enum Command:
    Add(title as Text)
    Done(id as Int)
    Show
    Help

error UsageError:
    message as Text
```

Функция, которая может завершиться ошибкой, говорит об этом в сигнатуре через `fails with`, а `fail` завершает её ошибкой. `match` разбирает значение по образцам: образец списка с остаточным элементом, охранное условие после `if`, альтернативы через `|`. `x is some(id)` проверяет один образец и связывает `id` в ветви, которую охраняет.

```unio
## Команда, которую называют аргументы; ошибка в них — это ошибка, а не сбой.
function parse_command(arguments as List[Text]) returns Command fails with UsageError:
    match arguments:
        case ["add", rest...] if not rest.is_empty(): return Add(title = join(rest, " "))
        case ["done", id_text]:
            if id_text.to_int() is some(id): return Done(id = id)
            fail UsageError(message = "номер задачи должен быть числом: {id_text}")
        case ["list"]: return Show
        case ["help"] | []: return Help
        case _: fail UsageError(message = "неизвестная команда: {join(arguments, " ")}")
```

Вызов функции, которая может завершиться ошибкой, никогда не бывает молчаливым. `try f()` передаёт ошибку дальше, и её должен перечислить `fails with` вызывающего; `try f() on E as e:` обрабатывает её на месте, и его блок должен покинуть инструкцию через `return`, `fail`, `break` или `continue`; `try:` с обработчиками `on` обрабатывает ошибки целого блока. Забытый `try` — E0073, ошибка, которая не обработана и не перечислена, — E0074. `match` должен покрывать каждый вариант (E0068), поэтому новая команда покажет все места, которые должны её обработать.

```unio
public function main() returns Int:
    let command = try parse_command(args.list()) on UsageError as e:
        print_error("Ошибка: {e.message}")
        return 2
    print("Программа бы: {describe(command)}")
    return 0
```

Ошибки — для того, что может пойти не так в правильной программе: неверный ввод, отсутствующий файл. Ошибка программиста — индекс за пределами, переполнение `Int`, невыполненный `assert` — это сбой: он завершает программу с кодом 101 и трассой, освободив открытые ресурсы, и перехватить его нельзя.

```sh
unio run todo_2_commands.unio -- add купить хлеб    # Программа бы: добавить "купить хлеб"
unio run todo_2_commands.unio -- done первая        # Ошибка: номер задачи должен быть числом: первая
```

## 5. Шаг 3: файл, JSON и разрешения

[todo_3_storage.unio](examples/todo_3_storage.unio) хранит задачи в `~/.todo.json`. Модули стандартной библиотеки импортируются по имени; `import std.json as json` связывает модуль с коротким именем, а публичные ошибки импортированного модуля, например `IoError` из `std.files`, видны по своим именам.

```unio
import std.env
import std.files
import std.json as json
```

`std.json` превращает структуру с публичными полями в JSON и обратно: `json.encode_checked` записывает список, а `json.decode[List[Task]]` называет тип, который строится из разобранного текста (`decode_tasks`). У каждой неудачи библиотеки свой тип ошибки, и программа переводит их в свою `StorageError`, которая называет файл:

```unio
function load_tasks(path as Path) returns List[Task] fails with StorageError:
    if not files.exists(path): return []
    let text = try files.read_text(path) on IoError as e:
        fail StorageError(path = path, reason = e.reason)
    return try decode_tasks(path, text)

function save_tasks(path as Path, tasks as List[Task]) fails with StorageError:
    let text = try json.encode_checked(tasks, pretty = true) on EncodeError as e:
        fail StorageError(path = path, reason = e.message)
    try files.write_text(path, text) on IoError as e:
        fail StorageError(path = path, reason = e.reason)
```

`files.write_text` заменяет файл атомарно: читатель видит старое или новое содержимое, но не смесь, как бы ни завершилась программа.

Программа может трогать только то, что разрешает её манифест. `unio check` выводит эффекты каждой функции из её вызовов (`files.read`, `files.write`, `env`, `process`) и сообщает о вызове, который манифест не разрешает, как E0085 с цепочкой вызовов, ведущей к нему; затем `unio run` выполняет программу в песочнице операционной системы ровно с этими разрешениями. Манифест примеров, [unio.toml](examples/unio.toml):

```toml
[package]
name = "todo"
version = "0.1.0"

[permissions]
files.read = ["$HOME/.todo.json"]
files.write = ["$HOME/.todo.json"]
env = ["TODO_FILE", "HOME"]
network = false
```

Тестам этого шага файл не нужен: они превращают список задач в текст JSON и обратно и проверяют, что повреждённый текст — это `StorageError` с именем файла.

## 6. Шаг 4: полная программа

[todo.unio](examples/todo.unio) соединяет три шага: `run` загружает задачи, выполняет команду и сохраняет список. `main` обрабатывает обе ошибки одним `try:` и превращает их в коды завершения; код завершения программы — её результат `Int` от 0 до 100.

```unio
public function main() returns Int:
    try:
        let command = parse_command(args.list())
        run(command, storage_path())
        return 0
    on UsageError as e:
        print_error("Ошибка: {e.message}")
        return 2
    on StorageError as e:
        print_error("Хранилище {e.path}: {e.reason}")
        return 1
```

Внутри блока `try:` вызовам, которые могут завершиться ошибкой, собственный `try` не нужен: их ошибки получают обработчики `on` ниже. Элемент списка меняется через список, `tasks[index].done = true`, потому что имя, связанное образцом, — копия, которую нельзя менять.

```sh
unio run todo.unio -- add купить хлеб
unio run todo.unio -- list
unio build todo.unio -o todo        # исполняемый файл с тем же поведением
./todo done 1
```

`unio build` компилирует программу через Cranelift и компонует её со средой выполнения в один исполняемый файл для машины, на которой работает; он применяет ту же песочницу из манифеста.

## 7. Пакеты и публикация

Пакет — каталог с `unio.toml`; каждый файл — модуль, `import tasks.store` — это файл `tasks/store.unio` того же пакета. Другой пакет — зависимость: он называется псевдонимом в `[dependencies]` и импортируется как `deps.<псевдоним>.<модуль>`. Команды полностью описаны в [packages.md](packages.md).

Библиотека `greetings` с одним модулем `words.unio`:

```unio
## Greetings.

public function greet(name as Text) returns Text:
    return "Hello, {name}"
```

Приложение добавляет её и импортирует через псевдоним:

```sh
cd app
unio add greetings --path ../greetings    # записывает [dependencies] и unio.lock
```

```unio
import deps.greetings.words

public function main():
    print(words.greet("Anna"))
```

`unio add`, `remove` и `update` подбирают версии и записывают `unio.lock` с точной версией и хэшем содержимого каждого пакета; файл блокировки фиксируется в репозитории. `unio install` устанавливает ровно то, что названо в файле блокировки, а `check`, `run`, `build` и `test` читают только файл блокировки и кэш пакетов и никогда не обращаются к сети. Установка пакета, модули которого помечены `unsafe module` или который объявляет разрешения, которых нет у приложения, требует подтверждения и ничего не выдаёт: программа всегда работает с разрешениями своего манифеста.

**Публикация.** Реестр — каталог подписанных файлов, который может раздавать любой статический HTTP-сервер. Пакеты подписываются ключами ML-DSA-65, и установка проверяет подпись по якорю доверия, который настроил оператор, прежде чем что-либо распаковать:

```sh
unio key new anchor --dir /media/offline              # ключ-якорь хранится вне сети
unio registry init /srv/registry --anchor /media/offline/anchor.key
unio key new signer                                   # в каталоге настроек unio
unio key endorse ~/.config/unio/keys/signer.pub --key /media/offline/anchor.key --registry /srv/registry
cd docs/examples
unio publish --registry /srv/registry --owner guide --key ~/.config/unio/keys/signer.key
```

Последняя команда публикует пакет текущего каталога, здесь `todo` 0.1.0, как `guide/todo`: она записывает детерминированный архив, его подпись и следующую подписанную версию индекса. Опубликованная версия никогда не заменяется и не удаляется. Пользователи указывают реестр в своих настройках с его URL и открытым ключом якоря ([packages.md](packages.md), раздел 5) и добавляют пакет командой `unio add todo 0.1.0 --package guide/todo --registry <имя>`.

## 8. От классов к Unio

В Unio нет классов, наследования и общих изменяемых объектов. Для каждой задачи, ради которой используют классы, есть своя конструкция, и [classes.unio](examples/classes.unio) показывает все их с тестами.

| С классами | В Unio |
|---|---|
| Класс с полями и методами | `struct` и блок `extend` |
| Конструктор | функция типа, `Account.open("Анна")`, или литерал структуры |
| Закрытое состояние | поля без `public`, видимые только в своём модуле |
| Базовый класс, абстрактный метод | `interface`; метод с блоком — реализация по умолчанию |
| Список объектов базового типа | список интерфейсного типа, `List[Shape]` |
| Переопределение | тот же метод в `extend T with I` |
| Наследование реализации | композиция: поле с другим значением и явное делегирование |
| Объекты, указывающие друг на друга | номера и словари, которыми владеет одно значение |
| `null` | `Option[T]` |
| Исключения | ошибки в `fails with`, которые обрабатывают `try` и `on` |

**Класс становится структурой и блоком extend.** Поля — это `struct`; методы — функции с `self` в `extend`. Метод, который меняет значение, принимает `var self` и вызывается только на месте `var`, поэтому каждое изменение видно. Конструктор — функция типа без `self`, вызываемая как `Account.open("Анна")`, и она может проверить аргументы. Поля без `public` закрыты в модуле: другой модуль не может ни создать структуру, ни изменить эти поля, и методы сохраняют свои инварианты.

```unio
struct Account:
    public owner as Text
    var balance as Int = 0
    var history as List[Int] = []

extend Account:
    ## Конструктор класса становится функцией типа: Account.open("Анна").
    public function open(owner as Text) returns Account:
        return Account(owner = owner)

    ## var self: метод меняет счёт, и место у вызывающего должно быть var.
    public function deposit(var self, amount as Int):
        self.balance = self.balance + amount
        self.history.append(amount)
```

**Базовый класс становится интерфейсом с реализациями по умолчанию.** Интерфейс объявляет методы, которые есть у каждой реализации. Метод с блоком — реализация по умолчанию: она общая для всех типов, реализующих интерфейс, и пользуется только методами самого интерфейса. Тип реализует интерфейс явно, в `extend T with I`, и может там заменить реализацию по умолчанию, как это сделало бы переопределение.

```unio
interface Shape:
    function area(self) returns Float
    function name(self) returns Text

    function describe(self) returns Text:
        return "{self.name()} площадью {self.area()}"
```

Интерфейс — это ещё и тип. `List[Shape]` хранит прямоугольники и квадраты вместе, а `shape.area()` при выполнении вызывает метод собственного типа каждого значения. Если у всех значений один тип, обобщённая функция `[T with Shape]` делает то же при компиляции. Приведения вниз нет: коду, которому нужно знать конкретный тип, подходит `enum`, чей `match` перечисляет все случаи.

```unio
function total_area(shapes as List[Shape]) returns Float:
    var total = 0.0
    for shape in shapes:
        total = total + shape.area()
    return total
```

**Наследование становится композицией.** Тип, который унаследовал бы другой, содержит его как поле. Делегирование записано явно, читатель видит, откуда берётся метод, и два типа не меняются вместе случайно:

```unio
struct Employee:
    public person as Person
    public team as Text

extend Employee:
    ## Делегирование записано явно, и читатель видит, откуда берётся имя.
    public function name(self) returns Text:
        return self.person.name
```

**Общие объекты становятся номерами и словарями.** Объектов, которые держат ссылки друг на друга — книги, знающей своего читателя, и читателя, знающего свои книги, — в Unio нет: значения копируются, а не разделяются. Данными владеет одно значение, здесь `Library` со словарём книг и словарём читателей, а части ссылаются друг на друга по номеру. Каждое изменение проходит через владельца, поэтому ничего не меняется за спиной другой части:

```unio
struct Library:
    var books as Map[Int, Book] = {}
    var members as Map[Int, Member] = {}
```

```unio
    public function lend(var self, book_id as Int, member_id as Int) fails with NotFound:
        if member_id not in self.members: fail NotFound(what = "читатель", id = member_id)
        match self.books.get(book_id):
            case some(book):
                var lent = book
                lent.borrower = some(member_id)
                self.books[book_id] = lent
            case none: fail NotFound(what = "книга", id = book_id)
```

**Типы предметной области — проверенные обёртки.** Значение, которое должно соблюдать правило, — порт, идентификатор, размер в байтах или длительность, — это структура с закрытым полем и конструктором, который его проверяет, поэтому каждое значение типа корректно и его нельзя спутать с простым числом:

```unio
struct Port:
    number as Int

extend Port:
    public function from_int(value as Int) returns Port fails with InvalidPort:
        if value < 1 or value > 65535: fail InvalidPort(value = value)
        return Port(number = value)
```

Что меняется в повседневной работе: равенство структур — по содержимому и даётся даром; копирование никогда не заметно, потому что значение копируется, только когда его меняют; `var` в сигнатуре говорит, какие аргументы функция меняет; а ресурс, например открытый файл, — не значение, он живёт в блоке `using`, который освобождает его, как бы блок ни завершился.

## 9. Что дальше

- [Справочник языка](reference.md): каждая конструкция и каждый код диагностики.
- [Справочник стандартной библиотеки](stdlib-reference.md), созданный из комментариев документации `stdlib/`.
- [Как Unio работает с ИИ](ai.md): форматы JSON и цикл «проверка, исправление, тесты».
- [Стиль Unio](style.md), [пакеты](packages.md), [среда выполнения](runtime.md).
