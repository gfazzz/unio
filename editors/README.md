# Editor support for Unio

| Editor | Directory | What it gives |
|---|---|---|
| VS Code, Cursor, and other VS Code forks | [vscode/](vscode) | Highlighting of `.unio` files and of ```` ```unio ```` blocks in Markdown, comments, brackets, indentation, folding |
| JetBrains IDEs | [vscode/](vscode) through *TextMate Bundles* | The same highlighting |
| Vim and Neovim | [vim/](vim) | Highlighting, four-space indentation, comments, folding by indentation |

Install the Vim files by adding the directory to the runtime path, for example `set runtimepath+=/path/to/unio/editors/vim` in `.vimrc` or `vim.opt.runtimepath:append("/path/to/unio/editors/vim")` in Neovim.

All editors follow the keyword lists of [spec/grammar.ebnf](../spec/grammar.ebnf), section L. The VS Code grammar has tests that fail when a keyword of the grammar is not highlighted (`cd vscode && npm install && npm test`); update the Vim file in the same change. The language server of requirement T-10 (diagnostics, go to definition, completion, rename) is built in stage 6 and will serve every editor that speaks LSP.
