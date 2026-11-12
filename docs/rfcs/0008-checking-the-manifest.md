# RFC 0008: `unio check` checks the manifest (E0088)

- Status: accepted (decided by the project lead on 2026-11-12; implemented in `compiler/unio/src/package/manifest.rs`, see Verification)
- Author: Unio project, with Claude
- Date: 2026-11-12
- Affects: requirements (T-6) / semantics (docs/semantics.md 8.6) / diagnostics (new code E0088) / tools
- Russian localization: [0008-checking-the-manifest.md](../ru/rfcs/0008-checking-the-manifest.md)

## Summary

`unio check` checks the manifest `unio.toml` of every package it checks as a whole, by the rules that `unio run`, `unio test`, and the package commands read it with (T-6), and reports each mistake as one diagnostic, E0088, at its line of the manifest. Until now only running found a mistake of the manifest, with code 103 and no line.

## Motivation

The R-10 model trial ([bench/results/control_trial-2026-11-12.md](../../bench/results/control_trial-2026-11-12.md)) found that `unio check` accepted `files.read = true`, which is not a list of scopes; `unio run` and `unio test` rejected it. Five answers of the first attempt failed that way: the agent saw a clean check and had no diagnostic to fix. Unio promises that `unio check` finds what is wrong before anything runs (C-2) and gives a machine-readable diagnostic with a place and a fix (C-8); the manifest is part of a package, so its mistakes belong to the check.

## Design

1. `unio check` reads the manifest of each package it reports (not of the standard library or of a dependency, whose manifest was checked when it was installed) and checks:
   - `[package]`: `name` (lowercase letters, digits, `-` and `_`, starting with a letter), `version` (a semantic version), `unsafe_modules` (a list of module names);
   - `[permissions]`: `files.read` and `files.write` as lists of paths whose placeholders are `$HOME`, `$CWD` and `$TEMP` and whose only `*` is the suffix `/**`, the same rule as the sandbox's own; `env` as a list of variable names; `process` as a list of programs; `network = false`;
   - `[dependencies]`: each entry a table with exactly one source (`path`; `registry` with `package` and `version`; or `archive` with `sha384`);
   - no other table or key; `[compile]` and `[limits]` belong to the external policy of the operator (RFC 0006), never to the manifest.
2. Each mistake is E0088 at its line of the manifest, once (semantics 2.8); a manifest that the reader of its format cannot read has only that mistake. In text output the place is `unio.toml:LINE:COLUMN`, in JSON `"file": "unio.toml"`.
3. One mistake, one message: a package whose manifest has a mistake gets no E0085, since its permissions are not known, and an import of a dependency whose entry has a mistake gets no E0048.
4. `unio run` and `unio test` keep stopping with code 103 before the program runs; the language server shows E0088 in `unio.toml` and, prefixed with its place, in each open document of the package.
5. Requirement T-6 gains the sentence: "`unio check` checks the manifest as a whole by these rules and reports each mistake as E0088 at its line." The catalog has E0088 with an explanation, an example and its fix.

## Migration

Not needed for valid manifests. A manifest with a mistake that only running found before now fails `unio check`; the diagnostic names the fix.

## Alternatives

- Keep the manifest out of the check and leave it to `unio run`: the agent and the person get no line and no fix, which is the failure the trial showed.
- Report a mistake of the manifest with an existing code (E0085 or E0048): those mean an effect without a permission and an import that cannot be resolved, so the message would mislead.
- Check only `[permissions]`: the other tables have the same problem, and the reader is shared.

## Verification

- `compiler/unio/tests/manifest.rs`: 32 malformed manifests, each one E0088 at its line; several mistakes each reported once; valid manifests give nothing; no E0085 or E0048 besides; text, JSON and Russian output and the exit code.
- `compiler/unio/tests/lsp.rs`: `a_mistake_of_the_manifest_is_a_diagnostic_of_the_package`.
- The control program `spec/tests/control/manifest_read_not_a_list/` (check A of R-10) and the catalog test of E0088.
