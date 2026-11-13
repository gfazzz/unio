// The VS Code and Cursor extension of Unio: highlighting comes from the grammars of
// package.json; this file starts the language server `unio lsp` (T-10) and connects it to the
// editor, and runs the tests of .unio files from the Testing view with `unio test` (T-11).
// Every analysis runs in a sandboxed worker of the server (SB-13), and every test in the
// sandbox of its package; the extension only passes texts and shows the results.
"use strict";

const vscode = require("vscode");
const { Client } = require("./client");
const testing = require("./testing");

let client = null;
let output = null;
let diagnostics = null;
// The diagnostics of the server behind those of the editor: their data names the revision of
// the document they were made for, and their fixes (T-13).
const original = new WeakMap();

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
                original.set(item, d);
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

// ---------------------------------------------------------------- tests (T-11)

function unioCommand() {
    const config = vscode.workspace.getConfiguration("unio");
    return config.get("server.path", "unio") || "unio";
}

/** The arguments of the sandbox that `unio test` shares with the server: --policy and the like. */
function sandboxArguments() {
    const config = vscode.workspace.getConfiguration("unio");
    return config.get("server.arguments", []).filter((a) => !a.startsWith("--lang"));
}

function setUpTests(context) {
    const controller = vscode.tests.createTestController("unio", "Unio");
    context.subscriptions.push(controller);

    // The tests of one file, found in its text: test blocks and documentation examples.
    function refresh(uri, text) {
        const key = uri.toString();
        const found = testing.discover(text);
        if (found.length === 0) {
            controller.items.delete(key);
            return;
        }
        let file = controller.items.get(key);
        if (!file) {
            file = controller.createTestItem(key, vscode.workspace.asRelativePath(uri), uri);
            controller.items.add(file);
        }
        file.children.replace(
            found.map((t) => {
                const item = controller.createTestItem(`${key}::${t.name}`, t.name, uri);
                item.range = new vscode.Range(t.line, 0, t.line, 0);
                item.description = t.kind === "doc" ? "documentation example" : undefined;
                return item;
            })
        );
    }

    async function refreshFile(uri) {
        try {
            const bytes = await vscode.workspace.fs.readFile(uri);
            refresh(uri, Buffer.from(bytes).toString("utf8"));
        } catch {
            controller.items.delete(uri.toString());
        }
    }

    controller.resolveHandler = async (item) => {
        if (!item) {
            const files = await vscode.workspace.findFiles("**/*.unio", "**/{node_modules,target}/**");
            await Promise.all(files.map(refreshFile));
        }
    };

    const watcher = vscode.workspace.createFileSystemWatcher("**/*.unio");
    context.subscriptions.push(
        watcher,
        watcher.onDidCreate(refreshFile),
        watcher.onDidChange(refreshFile),
        watcher.onDidDelete((uri) => controller.items.delete(uri.toString())),
        vscode.workspace.onDidChangeTextDocument((event) => {
            if (isUnio(event.document)) {
                refresh(event.document.uri, event.document.getText());
            }
        })
    );

    async function run(request, token) {
        const run = controller.createTestRun(request);
        // The files to run, each with the tests asked for (all when the file was asked for).
        const plan = new Map();
        const add = (item) => {
            const file = item.parent || item;
            if (!plan.has(file.id)) {
                plan.set(file.id, { file, tests: new Set() });
            }
            if (item.parent) {
                plan.get(file.id).tests.add(item);
            } else {
                item.children.forEach((child) => plan.get(file.id).tests.add(child));
            }
        };
        if (request.include) {
            request.include.forEach(add);
        } else {
            controller.items.forEach(add);
        }
        for (const { file, tests } of plan.values()) {
            if (token.isCancellationRequested) {
                break;
            }
            const wanted = [...tests].filter((t) => !(request.exclude || []).includes(t));
            wanted.forEach((t) => run.started(t));
            const path = file.uri.fsPath;
            const folder = vscode.workspace.getWorkspaceFolder(file.uri);
            // One test asked for: its name filters the run; otherwise the whole file runs.
            const name = wanted.length === 1 && wanted.length < file.children.size ? wanted[0].label : null;
            const done = await testing.runTests(unioCommand(), path, name, {
                cwd: folder ? folder.uri.fsPath : undefined,
                extra: sandboxArguments(),
                token,
            });
            if (done.stderr) {
                run.appendOutput(done.stderr.replace(/\n/g, "\r\n"));
            }
            const broken = done.results.find((r) => r.name === null);
            for (const t of wanted) {
                const result = done.results.find((r) => r.name === t.label);
                if (result && result.output) {
                    run.appendOutput(result.output.replace(/\n/g, "\r\n"), undefined, t);
                }
                if (result && result.passed) {
                    run.passed(t, result.duration);
                } else if (result) {
                    const message = new vscode.TestMessage(testing.failureMessage(result));
                    message.location = new vscode.Location(t.uri, t.range);
                    run.failed(t, message, result.duration);
                } else if (broken) {
                    run.errored(t, new vscode.TestMessage("the file does not compile; see the Problems view"));
                } else {
                    run.skipped(t);
                }
            }
        }
        run.end();
    }

    controller.createRunProfile("Run", vscode.TestRunProfileKind.Run, run, true);
}

/** The fixes of the server for the diagnostics of a range, for the revision they were made for. */
function codeActions() {
    return {
        async provideCodeActions(document, r, context) {
            const asked = context.diagnostics.map((d) => original.get(d)).filter((d) => d);
            if (!client || asked.length === 0) {
                return [];
            }
            // A diagnostic of an earlier revision has no fix until the checks ran again.
            if (asked.some((d) => d.data && d.data.revision !== undefined && d.data.revision !== document.version)) {
                return [];
            }
            const actions = await ask("textDocument/codeAction", {
                textDocument: { uri: document.uri.toString() },
                range: {
                    start: { line: r.start.line, character: r.start.character },
                    end: { line: r.end.line, character: r.end.character },
                },
                context: { diagnostics: asked },
            });
            return (actions || []).flatMap((a) => {
                const action = new vscode.CodeAction(a.title, vscode.CodeActionKind.QuickFix);
                const edit = new vscode.WorkspaceEdit();
                for (const change of (a.edit && a.edit.documentChanges) || []) {
                    if (change.textDocument.version !== document.version) {
                        return [];
                    }
                    for (const e of change.edits) {
                        edit.replace(vscode.Uri.parse(change.textDocument.uri), range(e.range), e.newText);
                    }
                }
                action.edit = edit;
                action.diagnostics = context.diagnostics;
                return [action];
            });
        },
    };
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
        vscode.languages.registerCodeActionsProvider(selector, codeActions(), {
            providedCodeActionKinds: [vscode.CodeActionKind.QuickFix],
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
    setUpTests(context);
}

function deactivate() {
    return stop();
}

module.exports = { activate, deactivate };
