# Packages and dependencies

Version: 0.1 (draft). Russian localization: [ru/packages.md](ru/packages.md). This document describes the package manager of the `unio` tool as implemented: the manifest, the lock file, the commands, the cache, the configuration of the operator, the signed registry served as static files with its keys and publishing, and the archive format. The normative requirements are T-6 to T-9 and T-14 of [requirements.md](requirements.md) with sections 5 and 6 of [language-contracts.md](language-contracts.md), and SB-10 to SB-15 for the isolation of the work; the workers are described in [runtime.md](runtime.md) 9.7. What is missing is listed at the end and in [status.md](status.md).

## 1. The manifest

A package is a directory with `unio.toml` (L-22). The package manager reads three tables:

```toml
[package]
name = "app"                 # lowercase letters, digits, '-' and '_'
version = "0.1.0"            # a semantic version; required for a package that others depend on
unsafe_modules = ["raw"]     # the modules that carry `unsafe module` (T-6)
unio_version = "^0.1"        # the versions of the Unio toolchain it needs (T-14); optional
targets = ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"]  # where it runs (T-14); optional

[dependencies]
util = { path = "../util" }
json = { registry = "main", package = "acme/json", version = "^1.2" }
vendored = { archive = "vendor/vendored-1.0.0.tar", sha384 = "<96 hexadecimal digits>" }

[permissions]
files.read = ["$CWD/**"]
```

`unio_version` and `targets` are the runtime and target requirements of the package (T-14, [language-contracts.md](language-contracts.md) section 6). `unio_version` is a version requirement in the syntax below on the Unio toolchain, the compiler and its runtime, matched against the release the toolchain belongs to: its version without a prerelease tag, so that a development build of 0.1.0 counts as 0.1.0. `targets` lists the targets the package runs on by the Rust triples of the targets of `unio build` (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`; [tools.md](tools.md) section 10); without the key the package runs everywhere, and an empty list runs nowhere. `unio add`, `remove`, `update` and `install` check both, for every package they install and for the root package, against this toolchain and this machine, before anything is written (section 3). A value of the wrong form (a requirement that does not parse, a target that is not one of the four, a target list that is not a list) is a mistake of the manifest, E0088.

The key of a dependency is its alias, the name in `import deps.<alias>.<module>` (T-14): lowercase letters, digits and `_`, not `std` or `deps`. A dependency has exactly one source:

- `path`: a directory, relative to the manifest that names it. Only the root package and other path dependencies may use it.
- `registry` and `package`: the package `owner/name` of a registry, with a `version` requirement. The registry is named by the operator's name for it (section 5); without `registry`, the root package uses the registry named `default`, and a package of a registry uses its own registry.
- `archive` and `sha384`: an archive file (relative to the manifest) or an `http://` URL on the host of a configured registry, with the SHA-384 of its bytes.

`version` is a requirement: `^1.2.3` (the default for a bare version), `~1.2`, `=1.2.3`, `>=1.0, <2.0`, `*`. A prerelease is chosen only when a requirement names a prerelease of the same version. The table form `[dependencies.util]` with `path = "../util"` below it means the same as the inline form.

`unio check` checks the whole manifest of each package it reports, as `unio run` and `unio test` read it: a value of the wrong form (`files.read = true` instead of a list of paths, an unknown placeholder or table, a dependency that is not a table or has no single source, an invalid name or version) is E0088 at its line of `unio.toml`, and the language server shows it too ([semantics.md](semantics.md) 8.6). `unsafe_modules` must name exactly the modules of the package that carry `unsafe module`; a marked module it does not name, or a name that is no marked module, is E0089 at its line (RFC 0004).

## 2. Identity, resolution, and the lock

The identity of a package is its origin (T-14): `registry+<registry URL>#<owner>/<name>`, `path+<directory relative to the root package>`, or `archive+<file relative to the root package, or URL>`. Two origins with the same name are two packages: their modules and types are different, even when their files are the same. Release 0.1 allows one version per identity.

Resolution (`unio add`, `remove`, `update`) is deterministic backtracking over the identities in sorted order, trying the versions that satisfy every requirement in descending semantic-version order. A package that is locked and not in the requested update keeps its locked version without asking the registry. When no version fits, the error names every path that requires the package (`the root package -> acme/b 1.0.0 requires ^1`), and for a kept version it suggests `unio update <alias>`. Two hidden copies of one package are never made.

`unio.lock` lists every package, direct and transitive, with its exact version, the content hash of its files, the SHA-384 of its archive, where the archive and its signature are downloaded from, the key whose signature of the archive was verified (`signer`), the hash and the sequence of the registry index it was chosen from (`index`, `index_sequence`; section 7.3), what the review of T-9 showed, and its own dependencies as `alias|requirement|identity`. Packages are sorted and every list is sorted, so the same resolution gives the same bytes; the format is versioned by `lock_version` (2; version 1, without `signer` and `index_sequence`, is still read). Commit the lock with the package.

The content hash is SHA-384 over a versioned canonical form of the files (`unio-package-content-v1`: every file in the byte order of its name, framed with its length). It names the entry of the package in the cache and binds the lock to the files that were reviewed. It authenticates nothing by itself ([cryptography.md](cryptography.md)).

## 3. Commands

| Command | What it does |
|---|---|
| `unio new NAME [--bin \| --lib]` | makes a new package in the directory NAME: the manifest (name, version `0.1.0`, no permissions), `main.unio` or a module named after the package with a `test` block, and `.gitignore` ([tools.md](tools.md) section 8) |
| `unio doc [--open] [-o DIR]` | writes the documentation of the package as HTML pages into `target/doc/` ([tools.md](tools.md) section 9) |
| `unio add ALIAS [VERSION] --path DIR` | adds a path dependency, resolves, installs, and writes `unio.toml` and `unio.lock` |
| `unio add ALIAS VERSION --package OWNER/NAME [--registry NAME]` | the same for a package of a registry |
| `unio add ALIAS --archive FILE\|URL --sha384 HEX` | the same for an archive with a known hash |
| `unio remove ALIAS` | removes the dependency and the packages nothing needs any more from the lock |
| `unio update [ALIAS...]` | resolves again, with the named dependencies (or all) free to change |
| `unio install` | installs exactly the packages of `unio.lock` into the cache; it never resolves and never changes the lock |
| `unio publish --registry DIR --owner OWNER --key KEY` | publishes the package of the working directory into a registry directory, signed (section 7.4) |
| `unio key new NAME [--dir DIR]`, `key id FILE` | makes an ML-DSA-65 key pair outside any package; prints the identity of a key file |
| `unio key endorse PUBLIC --key SIGNER --registry DIR [--expires DATE]`, `key revoke PUBLIC\|ID --key ANCHOR --registry DIR` | endorses a signing key, or revokes one, in the trust files of a registry |
| `unio registry init DIR --anchor ANCHOR`, `registry resign DIR --key KEY` | makes a new registry; signs every archive and index again after a key was revoked |

The commands from `add` to `install` take `--offline` (no download: a package that is not in the cache and needs the network is an error), `--unsafe-ok` (the confirmation of T-9, section 6), and `--policy FILE` and `--allow-weaker-sandbox` for its workers, as the other commands do. The commands look for `unio.toml` in the working directory and above it. Nothing is written until everything was fetched, checked, and confirmed: a failed or refused command leaves `unio.toml`, `unio.lock`, and the cache as they were. A path dependency is copied into the cache: after changing its files, run `unio update <alias>`; `unio install` reports the change instead of locking it silently.

`unio install` checks first that the lock matches the manifest (the same aliases, identities, and requirements, and every locked version satisfies the requirements on it); otherwise it stops and suggests `unio update`. For a package that is already in the cache it reads the entry again in a worker and checks its content hash.

Every package these commands install must meet its target and runtime requirements here (T-14): its `package.unio_version` must hold for this toolchain and its `package.targets`, when given, must name this machine. One that does not stops the command before anything is written, with what the package asks for and what this is: `unio install: error: path+../util 1.0.0 cannot be installed: it runs on aarch64-apple-darwin (package.targets), and this machine is x86_64-unknown-linux-gnu; nothing was installed`. The root package is checked the same way. A malformed key of a dependency's manifest is reported as E0088 with the manifest; `unio check` reports one in the package's own manifest at its line. Checked by `target_and_runtime_requirements_are_validated_at_installation` in `compiler/unio/tests/packages.rs` and the unit tests of `package/manifest.rs`.

`unio check`, `build`, `run`, `test`, and the language server read only `unio.lock` and the cache, never the network (T-7): `import deps.util.strings` resolves through the lock of the root package to the cache entry of `util`, and a module of a dependency imports the modules of its own package and its own dependencies the same way. An alias that is not declared, declared but not locked, or locked but not installed is E0048, and the message says which and what to run.

## 4. The cache

The cache is `UNIO_CACHE`, or the cache directory of the system (`$XDG_CACHE_HOME/unio` or `~/.cache/unio` on Linux, `~/Library/Caches/unio` on macOS, `%LOCALAPPDATA%\unio\cache` on Windows). An entry is `packages/<hex digest of the content hash>/` and holds the files of one package. Entries are made in `staging/` by the unpack worker and renamed into place by the tool once the package is checked and confirmed; an entry is never changed afterwards. The workers of the compiler may read only the entries that the lock of their package names (SB-11), never the whole cache. `trust/state` in the cache keeps the highest sequences of signed registry files seen on this machine (section 7.3).

## 5. The configuration of the operator

Registries and limits are set by the operator, never by a project: `UNIO_PACKAGES_CONFIG`, or `packages.toml` in the configuration directory (`$XDG_CONFIG_HOME/unio` or `~/.config/unio` on Linux, `~/Library/Application Support/unio` on macOS, `%APPDATA%\unio` on Windows). Unknown keys are errors.

```toml
[registry.main]
url = "http://registry.example.org/unio"
anchor = "keys/main.pub"    # the trust anchor of the registry, relative to this file (section 7.2)

[limits]
archive_bytes = 67108864    # the largest archive
unpacked_bytes = 67108864   # the files of one package together
file_bytes = 16777216       # one file
entries = 10000             # entries of one archive
depth = 32                  # directories in a name
```

The download worker may reach only the hosts of the configured registries (SB-12): an archive URL on another host is refused before anything is downloaded, and so is a package that must be downloaded from a registry the configuration does not list. Every registry needs `anchor`, the public key file of its trust anchor, which the operator obtains independently of the registry (from its operator, in person, or from a channel the operator trusts); a key that only the registry names is never trusted. The former `allow_unsigned`, which installed registry packages unverified, is refused with a message: no option accepts a registry package without a valid signature ([cryptography.md](cryptography.md), section 3: no exception may authorize package installation). Code that has no signature can still be used as an archive dependency, whose `sha384` the root manifest itself names.

## 6. The review of T-9

Installing a package that has modules marked `unsafe module` (listed in `unsafe_modules` or found by parsing its modules), or that declares permissions the root package does not have, needs an explicit confirmation. The tool shows, for each such package, the marked modules, that in 0.1 they contain no external functions (E0086 rejects them) and receive no wider permissions (RFC 0004), and the permissions it declares beyond the root's own; a module that carries the marker but is not listed, or is listed but does not carry it, is a warning. The confirmation is `--unsafe-ok`, or `yes` typed on a terminal; without it nothing is changed. `unio install` asks for every such package of the lock; `add` and `update` for the packages that are new or changed.

The confirmation grants nothing. The permissions of a dependency never widen the root package's: a program runs with the permissions of the manifest of its own package narrowed by the policy (SB-2), and the dependency's `[permissions]` are not read for the sandbox. Statically, a call from the root package into a dependency carries the effects of the dependency, so the root's manifest must permit them (E0085), while the modules of the dependency are checked against its own manifest.

## 7. A signed registry served as static files

A registry is a directory tree that any static HTTP server can serve. `unio registry init`, `unio key`, and `unio publish` write it on the publisher's machine; serving it, or copying it to a server, is up to the publisher (no upload service exists):

```text
<registry>/index/<owner>/<name>.json              the index of a package
<registry>/index/<owner>/<name>.json.sig          its signature envelope
<registry>/archives/<owner>/<name>/<version>.tar  the archive of a version (section 8)
<registry>/archives/<owner>/<name>/<version>.tar.sig
<registry>/trust/keys                             endorsements of signing keys
<registry>/trust/revocations                      the revocation list, signed by the anchor
<registry>/trust/anchor.pub                       the anchor's public key, for reference only
```

The index of a package:

```json
{
  "format": "unio-registry-index-2",
  "package": "acme/json",
  "sequence": 3,
  "versions": [
    {
      "version": "1.2.0",
      "archive": "archives/acme/json/1.2.0.tar",
      "sha384": "<96 hexadecimal digits of the archive>",
      "signature": "archives/acme/json/1.2.0.tar.sig",
      "dependencies": [{"alias": "text", "package": "acme/text", "version": "^1.0"}]
    }
  ]
}
```

`sequence` grows with every change of the index. Every version has a signature; format 1, which had none, is refused. Paths are relative to the registry and may not leave it (`..`, absolute paths, and other hosts are refused); dependencies name packages of the same registry. The manifest in the archive must have the name and version of the index entry and the same dependencies, or the package is refused. The client sends `GET` with `Connection: close`, follows no redirect, and limits the size of every response.

### 7.1 Signatures

Every signature of Unio is an envelope of nine ASCII lines ([cryptography.md](cryptography.md), section 7): the format `unio-envelope-1`, the purpose, the algorithm `ml-dsa-65` and the parameter set `fips204-ml-dsa-65`, the identity of the key (`sha384:` and the SHA-384 of its public key with a domain prefix), the subject, a sequence, the SHA-384 of the whole payload, and the ML-DSA-65 signature (FIPS 204, the pure variant with the context `unio-envelope-1`) of the eight lines before it. Another algorithm, parameter set, format, or purpose is refused, never verified some other way.

| Purpose | Payload | Subject | Sequence | Signed by |
|---|---|---|---|---|
| `package-archive` | the archive | `owner/name@version` | 0 | a trusted key |
| `registry-index` | the index document | `owner/name` | the index's `sequence` | a trusted key |
| `key-endorsement` | the endorsed key and its expiry | the key's identity | 0 | the anchor or a trusted key |
| `revocations` | the revocation list | `registry` | the list's sequence | the anchor only |
| `release-manifest` | the manifest of a release | the release | the release's number | the release key ([compiler/README.md](../compiler/README.md)) |

### 7.2 Trust

- The **anchor** is the key of `anchor` in the operator's configuration. Nothing the registry serves can replace it.
- **Endorsements** in `trust/keys` make other keys trusted: each names a public key and an optional expiry date (`expires 2027-06-30`, the first day it is no longer trusted, in UTC) and is signed by the anchor or by a key that is trusted itself, at most eight endorsements from the anchor. **Rotation** is an endorsement of the new key, by the old key or by the anchor.
- The **revocation list** in `trust/revocations` is signed by the anchor alone and has a sequence. A revoked key is not trusted, and neither is a key endorsed only through it, nor anything they signed. After a rotation by the old key, the anchor endorses the new key itself before it revokes the old one.
- A trust file whose signature does not hold, or that is malformed, refuses the whole installation.

### 7.3 What installation checks

1. The tool downloads `trust/keys` and `trust/revocations` and the envelope of the index as they are, and parses none of them.
2. The download worker checks the trust files against the anchor and the envelope of the index, with the lowest sequence it may accept, *before* it parses the index: a trusted key, the purpose `registry-index`, the subject of the package, the digest of exactly the downloaded bytes, and a sequence no lower than the floor; the document's `sequence` must equal the signed one.
3. A version that the lock records must still be in the index with the same archive hash: a registry that removed or replaced a published version is refused (T-8).
4. The unpack worker checks the SHA-384 of the archive against the index or the lock, then its envelope (`package-archive`, `owner/name@version`, the digest of the archive, a trusted key), and only then reads the archive.
5. `unio install` from a lock does the same for every package it downloads: a lock edited to name another archive, or a package whose key was revoked since, is refused.

Rollback protection: the floors are the highest sequences seen, of each index and of each registry's revocation list, kept in the cache (`trust/state`), and the `index_sequence` of the lock. An older index or revocation list, validly signed or not, is refused. A failure at any step leaves `unio.toml`, `unio.lock`, and the cache as they were. The lock records the signer and the sequence of every registry package. Packages already in the cache were verified when they were installed and are checked against their content hash.

### 7.4 Publishing and keys

`unio key new NAME` makes an ML-DSA-65 key pair from 32 bytes of the system's random source (if it fails, no key is made): `NAME.key`, the secret seed, readable by its owner only (on Linux and macOS; on Windows the file takes the permissions of its directory), and `NAME.pub`. The default directory is `keys/` in the configuration directory of Unio; a directory inside a package is refused, so that a key is never committed or published, and an existing key is never overwritten. The seed is overwritten with zeros when the tool is done with it. Signing is the deterministic variant of FIPS 204 and needs no randomness.

`unio publish --registry DIR --owner OWNER --key KEY` publishes the package of the working directory as `OWNER/<name>`: it makes the archive with the rules of a path dependency (section 8; the same files always give the same bytes, and with deterministic signing the same signature), refuses a package that contains a secret key file or a dependency that is not a package of the same registry with a version requirement, writes the archive and its envelope, and adds the version to the index with the next sequence and a new envelope. A version that is already published with the same content changes nothing; with other content it is refused: a published version is never replaced or removed, and no command deletes one (T-8). Yanking is not part of the requirements and does not exist.

The procedure of an operator:

```sh
unio key new anchor --dir /media/offline        # the anchor stays offline
unio registry init /srv/reg --anchor /media/offline/anchor.key
unio key new signer                              # in ~/.config/unio/keys/
unio key endorse ~/.config/unio/keys/signer.pub --key /media/offline/anchor.key --registry /srv/reg
cd mypackage && unio publish --registry /srv/reg --owner acme --key ~/.config/unio/keys/signer.key
```

Rotation: `unio key endorse new.pub --key signer.key` (or `--key anchor.key`), then publish with the new key. A compromised key: `unio key revoke signer.pub --key anchor.key --registry /srv/reg`, endorse a replacement with the anchor, and `unio registry resign /srv/reg --key replacement.key`, which checks every archive against its index and signs every archive and index again with the next sequences. Clients give the registry's anchor public key (`trust/anchor.pub` is the same file, for reference) to their operators by a channel they trust.

### 7.5 Transport

Only `http://` is supported in 0.1, by a decision of 2026-11-12: integrity, authenticity, and protection against substitution and rollback come from the signatures and the signed sequences above, whatever the transport. What the transport does not give is not given: an observer of the network sees which packages and versions are downloaded (no privacy), and an attacker on the path can withhold updates or keep serving the newest index the client has already seen (no freshness: indexes carry no signed time). TLS comes with the networking of 0.2, whose RFC defines the profiles ([cryptography.md](cryptography.md), section 5).

## 8. The archive format

A package archive is an uncompressed POSIX ustar archive, as `tar --format=ustar -cf pkg.tar -C pkg .` makes it; the files may also be below one top-level directory. It is a standard that existing tools write, its fixed 512-byte headers are parsed with the Rust standard library alone, and an uncompressed archive cannot expand: every byte of every file is present in the archive, so the unpacked size never exceeds the archive size, which the limits bound before anything is read, and no decompression-ratio attack exists. Compression is not required by the specification; a compressed format would be a new format version with its own ratio limit.

Accepted are regular files and directories with portable relative names: ASCII letters, digits, `.`, `_`, `-`, `+`. Refused, by the unpack worker and before anything is written: absolute names, `..`, backslashes and other characters, names Windows reserves or cannot keep (`CON`, `aux.unio`, a trailing dot), symbolic and hard links, character and block devices, FIFOs, pax extended headers and GNU long names, duplicate names, names that differ only in case (they collide on case-insensitive file systems, and a package must unpack the same everywhere), a file where a directory is needed, more entries than `entries`, deeper names than `depth`, files and packages over the size limits, a size that runs past the end of the archive, a bad header checksum, and data after the end of the archive. Files are written new, without execute or special bits. A path dependency is copied with the same rules (a link in it is refused), leaving out names that start with a dot and `target/` and `unio.lock` at its top.

## 9. What is not done

- HTTPS, and with it the privacy of downloads: 0.2 (section 7.5). No signed time for indexes, so withheld updates are not detected.
- An independent security review, fuzzing of the parsers of envelopes, trust files, and indexes (they are strict and bounded, and tested with modified and truncated inputs), and a review of the timing of the ML-DSA implementation: until then no component claims post-quantum compliance (R-6c; [cryptography.md](cryptography.md), section 7).
- One signature per object: no threshold of several keys; SLH-DSA as an independent alternative is not selected.
- An upload service: a registry is a directory that its publisher serves or copies.
- Yanking, which the requirements do not ask for, and removing old cache entries.
- On Windows each worker is a process of its own AppContainer without network (SB-4), and the download worker inherits the connection the tool opened, as on Linux and macOS ([runtime.md](runtime.md) 9.4, 9.7).
