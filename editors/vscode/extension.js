// The VS Code and Cursor extension of Unio: highlighting comes from the grammars of
// package.json; this file starts the language server `unio lsp` (T-10) and connects it to the
// editor. Every analysis runs in a sandboxed worker of the server (SB-13); the extension only
// passes the texts of the open documents and shows the results.
"use strict";

const vscode = require("vscode");
const { Client } = require("./client");

let client = null;
let output = null;
let diagnostics = null;

const selector = [{ language: "unio", scheme: "file" }];

function position(p) {
    return new vscode.Position(p.line, p.character);
}

function range(r) {
    return new vscode.Range(position(r.start), position(r.end));
}

function location(l) {
    return new vscode.Location(vscode.Uri.parse(l.uri), range(l.range));
}

function textEdit(e) {
    return vscode.TextEdit.replace(range(e.range), e.newText);
}

function at(document, p) {
    return {
        textDocument: { uri: document.uri.toString() },
        position: { line: p.line, character: p.character },
    };
}

function severity(s) {
    switch (s) {
        case 2:
            return vscode.DiagnosticSeverity.Warning;
        case 3:
            return vscode.DiagnosticSeverity.Information;
        case 4:
            return vscode.DiagnosticSeverity.Hint;
        default:
            return vscode.DiagnosticSeverity.Error;
    }
}

function isUnio(document) {
    return document.languageId === "unio" && document.uri.scheme === "file";
}

function open(document) {
    if (client && isUnio(document)) {
        client.notify("textDocument/didOpen", {
            textDocument: {
                uri: document.uri.toString(),
                languageId: "unio",
                version: document.version,
                text: document.getText(),
            },
        });
    }
}

function start(context) {
    const config = vscode.workspace.getConfiguration("unio");
    if (!config.get("server.enabled", true)) {
        return;
    }
    const command = config.get("server.path", "unio") || "unio";
    const args = config.get("server.arguments", []);
    const folder = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders[0];
    client = new Client(command, args, {
        cwd: folder ? folder.uri.fsPath : undefined,
        log: (text) => output.append(text),
    });
    const running = client;
    running.exited.then((code) => {
        if (client === running && code !== 0) {
            output.appendLine(`unio lsp ended with code ${code}`);
            vscode.window.showWarningMessage(
                "The Unio language server stopped; see the output 'Unio'. Run 'Unio: Restart the language server' to start it again."
            );
        }
    });
    client.on("textDocument/publishDiagnostics", (params) => {
        const uri = vscode.Uri.parse(params.uri);
        diagnostics.set(
            uri,
            params.diagnostics.map((d) => {
                const item = new vscode.Diagnostic(range(d.range), d.message, severity(d.severity));
                item.source = d.source || "unio";
                if (d.code) {
                    item.code = d.codeDescription
                        ? { value: d.code, target: vscode.Uri.parse(d.codeDescription.href) }
                        : d.code;
                }
                return item;
            })
        );
    });
    client
        .request("initialize", {
            processId: process.pid,
            clientInfo: { name: vscode.env.appName },
            locale: vscode.env.language,
            rootUri: folder ? folder.uri.toString() : null,
            capabilities: {},
        })
        .then(() => {
            client.notify("initialized", {});
            vscode.workspace.textDocuments.forEach(open);
        })
        .catch((error) => output.appendLine(`initialize: ${error.message}`));
}

async function stop() {
    if (client) {
        const running = client;
        client = null;
        await running.stop();
    }
    if (diagnostics) {
        diagnostics.clear();
    }
}

function ask(method, params) {
    return client ? client.request(method, params).catch((error) => {
        output.appendLine(`${method}: ${error.message}`);
        return null;
    }) : Promise.resolve(null);
}

function activate(context) {
    output = vscode.window.createOutputChannel("Unio");
    diagnostics = vscode.languages.createDiagnosticCollection("unio");
    context.subscriptions.push(output, diagnostics);

    context.subscriptions.push(
        vscode.workspace.onDidOpenTextDocument(open),
        vscode.workspace.onDidChangeTextDocument((event) => {
            if (client && isUnio(event.document)) {
                client.notify("textDocument/didChange", {
                    textDocument: { uri: event.document.uri.toString(), version: event.document.version },
                    contentChanges: [{ text: event.document.getText() }],
                });
            }
        }),
        vscode.workspace.onDidSaveTextDocument((document) => {
            if (client && isUnio(document)) {
                client.notify("textDocument/didSave", { textDocument: { uri: document.uri.toString() } });
            }
        }),
        vscode.workspace.onDidCloseTextDocument((document) => {
            if (client && isUnio(document)) {
                client.notify("textDocument/didClose", { textDocument: { uri: document.uri.toString() } });
            }
        }),
        vscode.languages.registerHoverProvider(selector, {
            async provideHover(document, p) {
                const hover = await ask("textDocument/hover", at(document, p));
                if (!hover) {
                    return null;
                }
                const text = new vscode.MarkdownString(hover.contents.value);
                return new vscode.Hover(text, hover.range ? range(hover.range) : undefined);
            },
        }),
        vscode.languages.registerDefinitionProvider(selector, {
            async provideDefinition(document, p) {
                const found = await ask("textDocument/definition", at(document, p));
                return found ? location(found) : null;
            },
        }),
        vscode.languages.registerCompletionItemProvider(
            selector,
            {
                async provideCompletionItems(document, p) {
                    const list = await ask("textDocument/completion", at(document, p));
                    if (!list) {
                        return [];
                    }
                    return list.items.map((i) => {
                        // LSP kinds are one more than those of VS Code.
                        const item = new vscode.CompletionItem(i.label, (i.kind || 1) - 1);
                        item.detail = i.detail;
                        return item;
                    });
                },
            },
            "."
        ),
        vscode.languages.registerDocumentFormattingEditProvider(selector, {
            async provideDocumentFormattingEdits(document, options) {
                const edits = await ask("textDocument/formatting", {
                    textDocument: { uri: document.uri.toString() },
                    options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
                });
                return edits ? edits.map(textEdit) : [];
            },
        }),
        vscode.languages.registerRenameProvider(selector, {
            async provideRenameEdits(document, p, newName) {
                if (!client) {
                    return null;
                }
                // A refusal (a name of the standard library, a keyword) is shown to the user.
                const result = await client.request("textDocument/rename", { ...at(document, p), newName });
                const edit = new vscode.WorkspaceEdit();
                for (const [uri, edits] of Object.entries(result.changes || {})) {
                    for (const e of edits) {
                        edit.replace(vscode.Uri.parse(uri), range(e.range), e.newText);
                    }
                }
                return edit;
            },
        }),
        vscode.languages.registerInlayHintsProvider(selector, {
            async provideInlayHints(document, r) {
                const hints = await ask("textDocument/inlayHint", {
                    textDocument: { uri: document.uri.toString() },
                    range: {
                        start: { line: r.start.line, character: r.start.character },
                        end: { line: r.end.line, character: r.end.character },
                    },
                });
                return (hints || []).map(
                    (h) => new vscode.InlayHint(position(h.position), h.label, vscode.InlayHintKind.Type)
                );
            },
        }),
        // The link of a diagnostic code: the explanation of `unio explain CODE`.
        vscode.workspace.registerTextDocumentContentProvider("unio-explain", {
            async provideTextDocumentContent(uri) {
                const code = uri.path.replace(/^\//, "");
                const text = await ask("unio/explain", { code });
                return text || `No explanation for ${code}.`;
            },
        }),
        vscode.commands.registerCommand("unio.restartServer", async () => {
            await stop();
            start(context);
        }),
        vscode.workspace.onDidChangeConfiguration(async (event) => {
            if (event.affectsConfiguration("unio.server")) {
                await stop();
                start(context);
            }
        })
    );
    start(context);
}

function deactivate() {
    return stop();
}

module.exports = { activate, deactivate };
