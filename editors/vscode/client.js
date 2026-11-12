// A minimal client of the language server protocol for `unio lsp`: JSON-RPC 2.0 over the
// standard streams of the server, with `Content-Length` framing. It has no dependency, so the
// extension stays without any (compiler/README.md keeps dependencies minimal), and it does not
// use the VS Code API, so its tests run under plain Node.js.
"use strict";

const { spawn } = require("node:child_process");

/** Splits the bytes of a stream into messages. */
class Framer {
    constructor(onMessage) {
        this.buffer = Buffer.alloc(0);
        this.onMessage = onMessage;
    }

    push(chunk) {
        this.buffer = Buffer.concat([this.buffer, chunk]);
        for (;;) {
            const end = this.buffer.indexOf("\r\n\r\n");
            if (end < 0) {
                return;
            }
            const headers = this.buffer.subarray(0, end).toString("ascii");
            const match = /content-length:\s*(\d+)/i.exec(headers);
            if (!match) {
                // Not a message: skip the headers.
                this.buffer = this.buffer.subarray(end + 4);
                continue;
            }
            const length = Number(match[1]);
            if (this.buffer.length < end + 4 + length) {
                return;
            }
            const body = this.buffer.subarray(end + 4, end + 4 + length).toString("utf8");
            this.buffer = this.buffer.subarray(end + 4 + length);
            this.onMessage(JSON.parse(body));
        }
    }
}

/** The bytes of one message. */
function frame(message) {
    const body = Buffer.from(JSON.stringify(message), "utf8");
    return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"), body]);
}

/** A running `unio lsp`. */
class Client {
    /**
     * @param {string} command the `unio` executable
     * @param {string[]} args the arguments after `lsp`
     * @param {{cwd?: string, env?: object, log?: (text: string) => void}} options
     */
    constructor(command, args, options = {}) {
        this.next = 1;
        this.waiting = new Map();
        this.handlers = new Map();
        this.log = options.log || (() => {});
        this.process = spawn(command, ["lsp", ...args], {
            cwd: options.cwd,
            env: options.env || process.env,
            stdio: ["pipe", "pipe", "pipe"],
        });
        const framer = new Framer((message) => this.receive(message));
        this.process.stdout.on("data", (chunk) => framer.push(chunk));
        // The server and its workers report on standard error: the log of the extension.
        this.process.stderr.on("data", (chunk) => this.log(chunk.toString("utf8")));
        this.exited = new Promise((resolve) => {
            this.process.on("exit", (code, signal) => {
                for (const { reject } of this.waiting.values()) {
                    reject(new Error(`the language server ended (${code ?? signal})`));
                }
                this.waiting.clear();
                resolve(code);
            });
            this.process.on("error", (error) => {
                this.log(`the language server cannot start: ${error.message}\n`);
                for (const { reject } of this.waiting.values()) {
                    reject(error);
                }
                this.waiting.clear();
                resolve(null);
            });
        });
    }

    receive(message) {
        if (message.id !== undefined && message.method === undefined) {
            const pending = this.waiting.get(message.id);
            if (pending) {
                this.waiting.delete(message.id);
                if (message.error) {
                    pending.reject(new Error(message.error.message));
                } else {
                    pending.resolve(message.result);
                }
            }
            return;
        }
        const handler = this.handlers.get(message.method);
        if (handler) {
            handler(message.params);
        }
    }

    /** Calls `handler` with the parameters of each notification of the method. */
    on(method, handler) {
        this.handlers.set(method, handler);
    }

    write(message) {
        if (this.process.stdin.writable) {
            this.process.stdin.write(frame(message));
        }
    }

    request(method, params) {
        const id = this.next++;
        return new Promise((resolve, reject) => {
            this.waiting.set(id, { resolve, reject });
            this.write({ jsonrpc: "2.0", id, method, params });
        });
    }

    notify(method, params) {
        this.write({ jsonrpc: "2.0", method, params });
    }

    /** Asks the server to shut down and exit; resolves with its exit code. */
    async stop() {
        try {
            await this.request("shutdown", null);
            this.notify("exit", null);
        } catch {
            this.process.kill();
        }
        return this.exited;
    }
}

module.exports = { Client, Framer, frame };
