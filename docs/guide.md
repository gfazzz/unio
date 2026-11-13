# The Unio guide

Version 0.1-draft. Russian localization: [ru/guide.md](ru/guide.md). Requirement D-2: from installation to publishing a package, with `todo` as the example, and a chapter on moving from classes to Unio.

The guide builds one program, `todo`, a command-line task list, in four steps. Each step is a file in [examples/](examples): [todo_1_tasks.unio](examples/todo_1_tasks.unio), [todo_2_commands.unio](examples/todo_2_commands.unio), [todo_3_storage.unio](examples/todo_3_storage.unio) and the complete [todo.unio](examples/todo.unio). The chapter on classes is [classes.unio](examples/classes.unio). Every one of them is checked by `unio check` and `unio fmt --check`, and its tests are run by `unio test`, in CI on Linux, macOS and Windows, so the code shown here works as written. The code in this guide is copied from those files.

The definition of the language is the [language reference](reference.md); the library is in the [standard library reference](stdlib-reference.md).

## 1. Installation

Release 0.1 is not published yet: `unio` is built from this repository. It needs the Rust toolchain that `compiler/rust-toolchain.toml` pins (rustup installs it on the first build) and the system linker that `unio build` uses: the C compiler `cc` on Linux, the Xcode command line tools on macOS, or the Visual Studio Build Tools with the C++ workload on Windows ([compiler/README.md](../compiler/README.md)).

```sh
git clone <URL of the Unio repository> unio && cd unio
sh .github/scripts/release_build.sh "$HOME/.local/unio"
export PATH="$HOME/.local/unio/bin:$PATH"
unio --help
```

The script builds the release with the pinned toolchain and the lock file and installs this layout:

```text
PREFIX/bin/unio                  the compiler, with the standard library embedded in it
PREFIX/lib/unio/libunio.a        the runtime of `unio build` (unio.lib on Windows)
PREFIX/share/doc/unio/           LICENSE-MIT, LICENSE-APACHE, THIRD-PARTY-NOTICES
```

`unio` needs nothing else at run time: the standard library is inside it, and `unio build` finds the runtime in `lib/unio/` next to `bin/`. Two builds in two directories give the same bytes. For an editor, install the extension of [editors/](../editors/README.md): highlighting, and the language server `unio lsp` with diagnostics, definitions, types, completion, formatting and rename.

## 2. The tools

One command does everything; each subcommand is described by `unio --help`.

| Command | What it does |
|---|---|
| `unio run FILE [-- ARGS]` | checks the program and runs it in the interpreter, inside the sandbox of its `unio.toml` |
| `unio check FILE...` | every static check without running anything; `--diagnostics=json` for tools and agents |
| `unio test FILE...` | runs the `test` blocks, each in isolation; `--json` for a machine-readable report |
| `unio fmt FILE...` | rewrites the files in the one style; `--check` only reports |
| `unio build FILE -o OUT` | a native executable for this machine; `--target TRIPLE` for another Linux machine, from Linux ([tools.md](tools.md) section 10) |
| `unio new NAME`, `unio new --lib NAME` | a new program or library package that checks, tests and formats as written |
| `unio doc [--open]` | the documentation of the package as HTML pages in `target/doc/`: documentation comments, signatures, errors and effects |
| `unio outline FILE`, `unio symbol MODULE.NAME` | what a file declares; one declaration with its documentation, errors and effects, and the interfaces a type implements with the method it uses for each |
| `unio explain CODE` | the explanation of a diagnostic code with an example and its fix |

A program is a file with `public function main()`. Its directory, or the nearest directory above it with `unio.toml`, is its package; the manifest names the package and grants it permissions. Without permissions a program may read its arguments and standard input and write to standard output and standard error, nothing more.

## 3. Step 1: tasks as values

[todo_1_tasks.unio](examples/todo_1_tasks.unio) keeps tasks in a list in memory. A task is a `struct`: a record with named, typed fields. `public` makes a field visible to other modules, and `var` lets it change; everything else is private and fixed.

```unio
struct Task:
    public id as Int
    public title as Text
    public var done as Bool = false
```

Functions state the types of their parameters and of their result. Inside a function, `let` and `var` infer the type of a local name. A lambda written where a function type is expected takes its types from there: `map` expects a function from `Task`, so `function (task): task.id` needs none. `or` gives a default for an `Option`: `max()` of an empty list is `none`.

```unio
## The number for a new task: one more than the largest number in the list.
function next_id(tasks as List[Task]) returns Int:
    return (tasks.map(function (task): task.id).max() or 0) + 1

## Adds a task to the caller's list; var makes the change visible at the call.
function add_task(var tasks as List[Task], title as Text):
    tasks.append(Task(id = next_id(tasks), title = title))
```

Values are values: a `Task` taken out of the list is a copy, and changing it does not change the list. A function changes its caller's data only through a `var` parameter, and the caller passes a `var` variable, so every change is visible at the call. An element changes through the list itself: `tasks[0].done = true`. Text is built by interpolation, `"[{mark}] {task.id}. {task.title}"`; there is no `+` between text and numbers.

Tests sit next to the code. `unio test` runs each `test` block in a fresh state:

```unio
test "a task is a value: a copy does not change the list":
    var tasks as List[Task] = []
    add_task(tasks, "a")
    var copy = tasks[0]
    copy.done = true
    assert not tasks[0].done
    tasks[0].done = true
    assert render(tasks[0]) == "[x] 1. a"
```

```sh
cd docs/examples
unio test todo_1_tasks.unio
unio run todo_1_tasks.unio
```

When something is wrong, `unio check` says what, where and how to fix it, with a code that `unio explain` describes. Writing `copy.done = true` with `let copy` instead of `var copy` gives E0078, a change through a place that is not mutable.

## 4. Step 2: commands and errors

[todo_2_commands.unio](examples/todo_2_commands.unio) reads a command from the arguments. The commands are an `enum`, a closed set of variants that may carry fields, and a wrong command is an `error`, a type of its own:

```unio
enum Command:
    Add(title as Text)
    Done(id as Int)
    Show
    Help

error UsageError:
    message as Text
```

A function that can fail says so in its signature with `fails with`, and `fail` ends it with an error. `match` takes a value apart by patterns: a list pattern with a rest element, a guard after `if`, alternatives with `|`. `x is some(id)` tests one pattern and binds `id` in the branch it guards.

```unio
## The command the arguments name; a mistake in them is an error, not a fault.
function parse_command(arguments as List[Text]) returns Command fails with UsageError:
    match arguments:
        case ["add", rest...] if not rest.is_empty(): return Add(title = join(rest, " "))
        case ["done", id_text]:
            if id_text.to_int() is some(id): return Done(id = id)
            fail UsageError(message = "task number must be an integer: {id_text}")
        case ["list"]: return Show
        case ["help"] | []: return Help
        case _: fail UsageError(message = "unknown command: {join(arguments, " ")}")
```

A call of a function that can fail is never silent. `try f()` passes the error on, which the caller's own `fails with` must list; `try f() on E as e:` handles it in place, and its block must leave the statement with `return`, `fail`, `break` or `continue`; `try:` with `on` handlers handles the errors of a whole block. Forgetting `try` is E0073, and an error that is neither handled nor listed is E0074. `match` must cover every variant (E0068), so adding a command later shows every place that has to handle it.

```unio
public function main() returns Int:
    let command = try parse_command(args.list()) on UsageError as e:
        print_error("Error: {e.message}")
        return 2
    print("Would {describe(command)}")
    return 0
```

Errors are for what can go wrong in a correct program: bad input, a missing file. A bug, such as an index out of range, an `Int` overflow or a failed `assert`, is a fault: it ends the program with exit code 101 and a trace, after releasing the open resources, and cannot be caught.

```sh
unio run todo_2_commands.unio -- add buy bread     # Would add "buy bread"
unio run todo_2_commands.unio -- done first        # Error: task number must be an integer: first
```

## 5. Step 3: a file, JSON, and permissions

[todo_3_storage.unio](examples/todo_3_storage.unio) keeps the tasks in `~/.todo.json`. Modules of the standard library are imported by name; `import std.json as json` binds the module under a short name, and the public errors of an imported module, such as `IoError` of `std.files`, are visible by their names.

```unio
import std.env
import std.files
import std.json as json
```

`std.json` turns a struct with public fields into JSON and back: `json.encode_checked` writes the list, and `json.decode[List[Task]]` names the type it builds from the parsed text (`decode_tasks`). Each failure of the library has its own error type, and the program translates them into its own `StorageError`, which names the file:

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

`files.write_text` replaces the file atomically: a reader sees the old content or the new one, never a mix, however the program ends.

A program may touch only what its manifest permits. `unio check` derives the effects of every function from what it calls (`files.read`, `files.write`, `env`, `process`) and reports a call that the manifest does not permit as E0085, with the chain of calls that leads to it; `unio run` then runs the program in an operating-system sandbox with exactly those permissions. The manifest of the examples, [unio.toml](examples/unio.toml):

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

The tests of this step need no file: they turn a list of tasks into JSON text and back, and check that a corrupted text is a `StorageError` that names the file.

## 6. Step 4: the complete program

[todo.unio](examples/todo.unio) joins the three steps: `run` loads the tasks, carries out the command, and saves the list. `main` handles both errors with one `try:` and turns them into exit codes; a program's exit code is its `Int` result, 0 to 100.

```unio
public function main() returns Int:
    try:
        let command = parse_command(args.list())
        run(command, storage_path())
        return 0
    on UsageError as e:
        print_error("Error: {e.message}")
        return 2
    on StorageError as e:
        print_error("Storage {e.path}: {e.reason}")
        return 1
```

Inside a `try:` block the calls that can fail need no `try` of their own: the `on` handlers below receive their errors. An element of the list changes through the list, `tasks[index].done = true`, because a name bound by a pattern is a copy that cannot change.

```sh
unio run todo.unio -- add buy bread
unio run todo.unio -- list
unio build todo.unio -o todo        # a native executable with the same behavior
./todo done 1
```

`unio build` compiles the program through Cranelift and links it with the runtime into one executable for the machine it runs on; it applies the same sandbox from the manifest.

## 7. Packages and publishing

A package is a directory with `unio.toml`; each file is a module, `import tasks.store` is the file `tasks/store.unio` of the same package. Another package is a dependency, named by an alias in `[dependencies]` and imported as `deps.<alias>.<module>`. The commands are described in full in [packages.md](packages.md).

`unio new app` makes a program package: `unio.toml`, `main.unio` with `main` and a `test` block, and a `.gitignore`; `unio new --lib greetings` makes a library whose module is `greetings.unio`. Both pass `unio check`, `unio test` and `unio fmt --check` as written. `unio doc` writes the documentation of the package into `target/doc/` (`--open` opens it): every public declaration with its signature, its documentation comment, and, for a function, its errors and the effects the compiler derives.

A library `greetings` with one module `words.unio`:

```unio
## Greetings.

public function greet(name as Text) returns Text:
    return "Hello, {name}"
```

An application adds it and imports it through its alias:

```sh
cd app
unio add greetings --path ../greetings    # writes [dependencies] and unio.lock
```

```unio
import deps.greetings.words

public function main():
    print(words.greet("Anna"))
```

`unio add`, `remove` and `update` resolve versions and write `unio.lock` with the exact version and content hash of every package; commit the lock. `unio install` installs exactly what the lock names, and `check`, `run`, `build` and `test` read only the lock and the package cache, never the network. Installing a package whose modules carry `unsafe module`, or that declares permissions the application does not have, asks for confirmation and grants nothing: a program always runs with the permissions of its own manifest.

**Publishing.** A registry is a directory of signed files that any static HTTP server can serve. Packages are signed with ML-DSA-65 keys, and an installation verifies the signature against the trust anchor that the operator configured before it unpacks anything:

```sh
unio key new anchor --dir /media/offline              # the anchor key stays offline
unio registry init /srv/registry --anchor /media/offline/anchor.key
unio key new signer                                   # in the configuration directory of unio
unio key endorse ~/.config/unio/keys/signer.pub --key /media/offline/anchor.key --registry /srv/registry
cd docs/examples
unio publish --registry /srv/registry --owner guide --key ~/.config/unio/keys/signer.key
```

The last command publishes the package of the current directory, here `todo` 0.1.0, as `guide/todo`: it writes a deterministic archive, its signature, and the next signed version of the index. A published version is never replaced or deleted. Users list the registry in their own configuration with its URL and anchor public key ([packages.md](packages.md) section 5) and add the package with `unio add todo 0.1.0 --package guide/todo --registry <name>`.

## 8. From classes to Unio

Unio has no classes, no inheritance and no shared mutable objects. Each thing classes are used for has its own construct, and [classes.unio](examples/classes.unio) shows all of them with tests.

| With classes | In Unio |
|---|---|
| A class with fields and methods | a `struct` and an `extend` block |
| A constructor | a function of the type, `Account.open("Anna")`, or the struct literal |
| Private state | fields without `public`, visible only in their module |
| A base class, an abstract method | an `interface`; a method with a block is a default |
| A list of objects of a base type | a list of the interface type, `List[Shape]` |
| An override | the same method in `extend T with I` |
| Inheritance of implementation | composition: a field that holds the other value, and written-out delegation |
| Objects that point at each other | identifiers and maps owned by one value |
| `null` | `Option[T]` |
| Exceptions | errors in `fails with`, handled by `try` and `on` |

**A class becomes a struct and an extend block.** The fields are a `struct`; the methods are functions with `self` in `extend`. A method that changes the value takes `var self`, and can be called only on a `var` place, so every change is visible. A constructor is a function of the type without `self`, called as `Account.open("Anna")`, and it can check its arguments. Fields without `public` are private to the module: another module cannot build the struct or change those fields, so the methods keep their invariants.

```unio
struct Account:
    public owner as Text
    var balance as Int = 0
    var history as List[Int] = []

extend Account:
    ## The constructor of a class becomes a function of the type: Account.open("Anna").
    public function open(owner as Text) returns Account:
        return Account(owner = owner)

    ## var self: the method changes the account, and the caller's place must be a var.
    public function deposit(var self, amount as Int):
        self.balance = self.balance + amount
        self.history.append(amount)
```

**A base class becomes an interface with defaults.** An interface declares the methods every implementation has. A method with a block is a default: it is shared by every type that implements the interface and may use only the interface's own methods. A type implements an interface explicitly, in `extend T with I`, and may replace a default there, as an override would.

```unio
interface Shape:
    function area(self) returns Float
    function name(self) returns Text

    function describe(self) returns Text:
        return "{self.name()} of area {self.area()}"
```

An interface is also a type. A `List[Shape]` holds rectangles and squares together, and `shape.area()` calls the method of each value's own type at run time. When all values have one type, a generic function `[T with Shape]` does the same at compile time. There is no downcast: code that has to know the concrete type uses an `enum`, whose `match` lists every case.

```unio
function total_area(shapes as List[Shape]) returns Float:
    var total = 0.0
    for shape in shapes:
        total = total + shape.area()
    return total
```

**Inheritance becomes composition.** A type that would extend another contains it as a field. Delegation is written out, so a reader sees where a method comes from, and the two types do not change together by accident:

```unio
struct Employee:
    public person as Person
    public team as Text

extend Employee:
    ## Delegation is written out, so a reader sees where the name comes from.
    public function name(self) returns Text:
        return self.person.name
```

**Shared objects become identifiers and maps.** Objects that hold references to each other, a book that knows its borrower and a member who knows their books, do not exist in Unio, because values are copied, not shared. One value owns the data, here a `Library` with a map of books and a map of members, and the parts refer to each other by number. Every change goes through the owner, so nothing changes behind another part's back:

```unio
struct Library:
    var books as Map[Int, Book] = {}
    var members as Map[Int, Member] = {}
```

```unio
    public function lend(var self, book_id as Int, member_id as Int) fails with NotFound:
        if member_id not in self.members: fail NotFound(what = "member", id = member_id)
        match self.books.get(book_id):
            case some(book):
                var lent = book
                lent.borrower = some(member_id)
                self.books[book_id] = lent
            case none: fail NotFound(what = "book", id = book_id)
```

**Domain types are checked wrappers.** A value that must satisfy a rule, a port, an identifier, a size in bytes or a duration, is a struct with a private field and a constructor that checks it, so every value of the type is valid and cannot be confused with a plain number:

```unio
struct Port:
    number as Int

extend Port:
    public function from_int(value as Int) returns Port fails with InvalidPort:
        if value < 1 or value > 65535: fail InvalidPort(value = value)
        return Port(number = value)
```

What changes in daily work: equality of structs is by content and comes for free; copying is never visible, because a value is copied only when it is changed; `var` in a signature says which arguments a function changes; and a resource such as an open file is not a value but lives in a `using` block, which releases it however the block ends.

## 9. Where next

- [Language reference](reference.md): every construct and every diagnostic code.
- [Standard library reference](stdlib-reference.md), generated from the documentation comments of `stdlib/`.
- [How Unio works with AI](ai.md): the JSON formats and the loop of check, fix and test.
- [The Unio style](style.md), [packages](packages.md), [the runtime](runtime.md).
