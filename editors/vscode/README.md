# Unio for VS Code and Cursor

Syntax highlighting for the [Unio](https://github.com/gfazzz/unio) language: `.unio` files and ```` ```unio ```` blocks in Markdown. Comment toggling, bracket pairs, indentation after `:`, and folding by indentation are included. The language server (diagnostics, go to definition, completion) comes with stage 6 of the plan.

## Install

Build the package once (Node.js 18 or newer):

```sh
cd editors/vscode
npx @vscode/vsce package
```

Then install `unio-0.1.2.vsix`:

- **Cursor:** `cursor --install-extension unio-0.1.2.vsix`, or the command palette: *Extensions: Install from VSIX...*
- **VS Code:** `code --install-extension unio-0.1.2.vsix`, or the same command.
- **JetBrains IDEs:** *Settings, Editor, TextMate Bundles*, add the `editors/vscode` directory.

Without building a package, the directory can be linked into the extensions folder: `ln -s "$PWD" ~/.cursor/extensions/unio.unio-0.1.2` (or `~/.vscode/extensions/...`), then reload the window. The CI job `editors` also builds the package as a downloadable artifact.

## Development

```sh
npm install
npm test
```

The tests tokenize Unio with the TextMate engine that VS Code and Cursor use, check the scopes of keywords, contextual keywords, text with interpolation, numbers, and comments, check that every keyword of `spec/grammar.ebnf` is highlighted, and tokenize every `.unio` file of the repository. A new keyword in the grammar makes the test fail until the highlighting knows it.

## License

Same as the Unio repository.
