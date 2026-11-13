# Unio for VS Code and Cursor

Syntax highlighting for the [Unio](https://github.com/gfazzz/unio) language: `.unio` files and ```` ```unio ```` blocks in Markdown. Comment toggling, bracket pairs, indentation after `:`, and folding by indentation are included.

The extension starts the language server `unio lsp` for `.unio` files: diagnostics with their codes (the code links to its explanation), quick fixes of the diagnostics, offered only for the revision of the document they were made for, go to definition, hover with the type of a name and the signature, errors, and effects of a function, type hints after bindings written without a type, completion of names, members, and keywords, *Format Document*, and *Rename Symbol* across the package.

The Testing view lists the tests of every `.unio` file of the workspace: its `test` blocks and the examples of its documentation comments, found in the text as `unio test` finds them. Running a file or one test runs `unio test --json` (the executable of `unio.server.path`, with the sandbox arguments of `unio.server.arguments`) in the sandbox of the package and shows each result: passed, failed with its message and output and, for a failing property, the command that replays it, or errored when the file does not compile ([docs/tools.md](../../docs/tools.md), section 7). Every analysis runs in a sandboxed worker of `unio` that reads only the package of the document and the standard library (SB-13), so an untrusted project opened in the editor cannot read, write, or run anything else. The client is `client.js`, without dependencies.

Settings: `unio.server.path` (the `unio` executable, by default `unio` on the search path), `unio.server.arguments` (for example `["--policy", "/path/to/policy.toml"]`, whose `[compile]` table limits each analysis; on Windows `["--allow-weaker-sandbox"]` until the sandbox there is complete), and `unio.server.enabled`. The command *Unio: Restart the language server* starts it again; its log is the output channel *Unio*.

## Install

Build the package once (Node.js 18 or newer):

```sh
cd editors/vscode
npx @vscode/vsce package
```

Then install `unio-0.3.0.vsix`:

- **Cursor:** `cursor --install-extension unio-0.3.0.vsix`, or the command palette: *Extensions: Install from VSIX...*
- **VS Code:** `code --install-extension unio-0.3.0.vsix`, or the same command.
- **JetBrains IDEs:** *Settings, Editor, TextMate Bundles*, add the `editors/vscode` directory.

Without building a package, the directory can be linked into the extensions folder: `ln -s "$PWD" ~/.cursor/extensions/unio.unio-0.3.0` (or `~/.vscode/extensions/...`), then reload the window. The CI job `editors` also builds the package as a downloadable artifact.

## Development

```sh
npm install
npm test
```

The tests check the framing of the client and, when `compiler/target` holds a built `unio`, a session with `unio lsp` (with a quick fix bound to its revision); they check the discovery of tests, the arguments of `unio test`, the reading of its JSON and, with a built `unio`, a run whose results are matched to the discovered tests (`testing.js`); they tokenize Unio with the TextMate engine that VS Code and Cursor use, check the scopes of keywords, contextual keywords, text with interpolation, numbers, and comments, check that every keyword of `spec/grammar.ebnf` is highlighted, and tokenize every `.unio` file of the repository. A new keyword in the grammar makes the test fail until the highlighting knows it.

## License

Same as the Unio repository.
