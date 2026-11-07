// Benchmark (b), the language's own JSON library.
const fs = require("fs");
const value = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
console.log(Buffer.byteLength(JSON.stringify(value), "utf8"));
