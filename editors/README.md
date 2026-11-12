# Editor support for Unio

| Editor | Directory | What it gives |
|---|---|---|
| VS Code, Cursor, and other VS Code forks | [vscode/](vscode) | Highlighting of `.unio` files and of ```` ```unio ```` blocks in Markdown, comments, brackets, indentation, folding; the language server `unio lsp` |
| JetBrains IDEs | [vscode/](vscode) through *TextMate Bundles* | The same highlighting |
| Vim and Neovim | [vim/](vim) | Highlighting, four-space indentation, comments, folding by indentation |

Install the Vim files by adding the directory to the runtime path, for example `set runtimepath+=/path/to/unio/editors/vim` in `.vimrc` or `vim.opt.runtimepath:append("/path/to/unio/editors/vim")` in Neovim.

All editors follow the keyword lists of [spec/grammar.ebnf](../spec/grammar.ebnf), section L. The VS Code grammar has tests that fail when a keyword of the grammar is not highlighted (`cd vscode && npm install && npm test`); update the Vim file in the same change. The language server of requirement T-10 is `unio lsp`: it speaks LSP on standard input and output and serves every editor that speaks LSP (Neovim with `vim.lsp.start({ name = "unio", cmd = { "unio", "lsp" } })`, Helix, Zed, Emacs `eglot`). It gives diagnostics with their codes, go to definition, hover with types, signatures, and effects, type hints, completion, formatting, and rename. The server reads no source itself: every analysis runs in a sandboxed worker that may read only the package of the document and the standard library, with the limits of the `[compile]` table of the policy (`unio lsp --policy FILE`), so opening an untrusted project is no more dangerous than reading its files (SB-13, [docs/runtime.md](../docs/runtime.md) 9.7). The VS Code extension starts it by itself.
