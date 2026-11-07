// Benchmark (a): lines, words, and UTF-8 bytes of a large text.
const fs = require("fs");
const text = fs.readFileSync(process.argv[2], "utf8");
const lines = text.split("\n").length - 1;
const words = text.split(/\s+/).filter((w) => w.length > 0).length;
console.log(`${lines} ${words} ${Buffer.byteLength(text, "utf8")}`);
