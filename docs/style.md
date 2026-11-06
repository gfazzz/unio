# The Unio style

Version 0.1-draft. Russian localization: [ru/style.md](ru/style.md). Basis: requirements T-1, T-1a, T-2, T-3; concept section 1 (one task, one way to write it).

Unio has one style and no settings. `unio fmt` writes every source in this form, and `unio fmt --check` fails for a source that differs from it. The rules decide every choice that does not change meaning, so that two people, or a person and an AI agent, who write the same program get the same text.

## 1. Layout

1. UTF-8, line ends LF, one line end at the end of the file, no trailing white space.
2. Indentation is 4 spaces per block level; tabs are errors (E0001).
3. The line width is 100 characters, counted in Unicode characters. Comments and text literals are never broken; a line that is longer only because of them stays long.

## 2. Blank lines

1. Nothing comes before the module documentation comment and `unsafe module`. After the module header, one blank line.
2. Imports form one group without blank lines: first `import` lines sorted by module path, then `from` lines sorted by module path, with the imported names of each line sorted. One blank line after the group. Sorting is the only change of order the formatter makes; comments between imports keep the import they precede.
3. Exactly one blank line between top-level declarations and `test` blocks. Consecutive top-level constants (`let`) keep the blank lines between them, at most one.
4. Inside a block, at most one blank line between statements, as in the source; none at the start or the end of a block.
5. Exactly one blank line between the members of an `extend` block and between interface methods that have a body; none between fields, variants, and bodiless interface methods unless the source has one (at most one).

## 3. Spaces

1. One space around binary operators (`+`, `==`, `and`, `in`, `is`, `**`, `->`) and around `=` in bindings, assignments, defaults, and named arguments: `connect(host, port = 8080)`.
2. One space after `,` and after `:` in maps; no space before them.
3. No space inside brackets, before `(` of a call or `[` of an index or of generic arguments, after a unary `-`, or around `.` and `?.`.
4. `as`, `returns`, `fails with`, `with`, and `on` are surrounded by single spaces.

## 4. Breaking long lines

1. A construct is written on one line if the line fits the width.
2. Otherwise the outermost bracketed group that ends last on the line is broken: its opening bracket ends the line, each item stands on its own line one level deeper and ends with `,`, and the closing bracket stands on its own line at the original level. The rule is then applied to each new line.
3. A line that has no bracketed group to break stays as it is.
4. Trailing commas exist only in broken groups. The form of a group never depends on how the source wrote it.

## 5. Branches

A branch of `if`, `else if`, `else`, `case`, or `on` that holds one simple statement is written on its header line when the line fits the width, and on the next line otherwise (T-1a). Functions, loops, `using`, `test`, `try:`, and statements with an inline handler always have a block.

## 6. Comments and literals

1. Comments and documentation comments are kept with their text; trailing white space is removed. A comment on its own line keeps its place before the statement or declaration that follows it, at that statement's indentation. A comment at the end of a code line is separated from the code by two spaces.
2. A documentation comment stands directly before its declaration, without a blank line.
3. Text literals, numbers, and escapes are written exactly as in the source.
4. Parentheses are kept as written.

## 7. Guarantees

1. Idempotence: formatting a formatted source changes nothing (T-1).
2. Meaning is kept: the syntax tree of the result equals the syntax tree of the source, apart from the order of imports, which rule 2.2 sorts (T-2).
3. Speed: a 1,000-line source is formatted in at most 50 ms (T-3).
4. A source with a syntax error is not formatted; `unio fmt` reports the errors and leaves the file unchanged.
