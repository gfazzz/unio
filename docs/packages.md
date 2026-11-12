# Packages and dependencies

Version: 0.1 (draft). Russian localization: [ru/packages.md](ru/packages.md). This document describes the package manager of the `unio` tool as implemented: the manifest, the lock file, the commands, the cache, the configuration of the operator, the static layout of a registry, and the archive format. The normative requirements are T-6 to T-9 and T-14 of [requirements.md](requirements.md) with sections 5 and 6 of [language-contracts.md](language-contracts.md), and SB-10 to SB-15 for the isolation of the work; the workers are described in [runtime.md](runtime.md) 9.7. What is missing is listed at the end and in [status.md](status.md).

## 1. The manifest

A package is a directory with `unio.toml` (L-22). The package manager reads three tables:

```toml
[package]
name = "app"                 # lowercase letters, digits, '-' and '_'
version = "0.1.0"            # a semantic version; required for a package that others depend on
unsafe_modules = ["raw"]     # the modules that carry `unsafe module` (T-6)

[dependencies]
util = { path = "../util" }
json = { registry = "main", package = "acme/json", version = "^1.2" }
vendored = { archive = "vendor/vendored-1.0.0.tar", sha384 = "<96 hexadecimal digits>" }

[permissions]
files.read = ["$CWD/**"]
```

The key of a dependency is its alias, the name in `import deps.<alias>.<module>` (T-14): lowercase letters, digits and `_`, not `std` or `deps`. A dependency has exactly one source:

- `path`: a directory, relative to the manifest that names it. Only the root package and other path dependencies may use it.
- `registry` and `package`: the package `owner/name` of a registry, with a `version` requirement. The registry is named by the operator's name for it (section 5); without `registry`, the root package uses the registry named `default`, and a package of a registry uses its own registry.
- `archive` and `sha384`: an archive file (relative to the manifest) or an `http://` URL on the host of a configured registry, with the SHA-384 of its bytes.

`version` is a requirement: `^1.2.3` (the default for a bare version), `~1.2`, `=1.2.3`, `>=1.0, <2.0`, `*`. A prerelease is chosen only when a requirement names a prerelease of the same version. The table form `[dependencies.util]` with `path = "../util"` below it means the same as the inline form.

## 2. Identity, resolution, and the lock

The identity of a package is its origin (T-14): `registry+<registry URL>#<owner>/<name>`, `path+<directory relative to the root package>`, or `archive+<file relative to the root package, or URL>`. Two origins with the same name are two packages: their modules and types are different, even when their files are the same. Release 0.1 allows one version per identity.

Resolution (`unio add`, `remove`, `update`) is deterministic backtracking over the identities in sorted order, trying the versions that satisfy every requirement in descending semantic-version order. A package that is locked and not in the requested update keeps its locked version without asking the registry. When no version fits, the error names every path that requires the package (`the root package -> acme/b 1.0.0 requires ^1`), and for a kept version it suggests `unio update <alias>`. Two hidden copies of one package are never made.

`unio.lock` lists every package, direct and transitive, with its exact version, the content hash of its files, the SHA-384 of its archive, where the archive and its signature are downloaded from, the hash of the registry index it was chosen from, what the review of T-9 showed, and its own dependencies as `alias|requirement|identity`. Packages are sorted and every list is sorted, so the same resolution gives the same bytes; the format is versioned by `lock_version`. Commit the lock with the package.

The content hash is SHA-384 over a versioned canonical form of the files (`unio-package-content-v1`: every file in the byte order of its name, framed with its length). It names the entry of the package in the cache and binds the lock to the files that were reviewed. It authenticates nothing by itself ([cryptography.md](cryptography.md)).

## 3. Commands

| Command | What it does |
|---|---|
| `unio add ALIAS [VERSION] --path DIR` | adds a path dependency, resolves, installs, and writes `unio.toml` and `unio.lock` |
| `unio add ALIAS VERSION --package OWNER/NAME [--registry NAME]` | the same for a package of a registry |
| `unio add ALIAS --archive FILE\|URL --sha384 HEX` | the same for an archive with a known hash |
| `unio remove ALIAS` | removes the dependency and the packages nothing needs any more from the lock |
| `unio update [ALIAS...]` | resolves again, with the named dependencies (or all) free to change |
| `unio install` | installs exactly the packages of `unio.lock` into the cache; it never resolves and never changes the lock |

Every command takes `--offline` (no download: a package that is not in the cache and needs the network is an error), `--unsafe-ok` (the confirmation of T-9, section 6), and `--policy FILE` and `--allow-weaker-sandbox` for its workers, as the other commands do. The commands look for `unio.toml` in the working directory and above it. Nothing is written until everything was fetched, checked, and confirmed: a failed or refused command leaves `unio.toml`, `unio.lock`, and the cache as they were. A path dependency is copied into the cache: after changing its files, run `unio update <alias>`; `unio install` reports the change instead of locking it silently.

`unio install` checks first that the lock matches the manifest (the same aliases, identities, and requirements, and every locked version satisfies the requirements on it); otherwise it stops and suggests `unio update`. For a package that is already in the cache it reads the entry again in a worker and checks its content hash.

`unio check`, `build`, `run`, `test`, and the language server read only `unio.lock` and the cache, never the network (T-7): `import deps.util.strings` resolves through the lock of the root package to the cache entry of `util`, and a module of a dependency imports the modules of its own package and its own dependencies the same way. An alias that is not declared, declared but not locked, or locked but not installed is E0048, and the message says which and what to run.

## 4. The cache

The cache is `UNIO_CACHE`, or the cache directory of the system (`$XDG_CACHE_HOME/unio` or `~/.cache/unio` on Linux, `~/Library/Caches/unio` on macOS, `%LOCALAPPDATA%\unio\cache` on Windows). An entry is `packages/<hex digest of the content hash>/` and holds the files of one package. Entries are made in `staging/` by the unpack worker and renamed into place by the tool once the package is checked and confirmed; an entry is never changed afterwards. The workers of the compiler may read only the entries that the lock of their package names (SB-11), never the whole cache.

## 5. The configuration of the operator

Registries and limits are set by the operator, never by a project: `UNIO_PACKAGES_CONFIG`, or `packages.toml` in the configuration directory (`$XDG_CONFIG_HOME/unio` or `~/.config/unio` on Linux, `~/Library/Application Support/unio` on macOS, `%APPDATA%\unio` on Windows). Unknown keys are errors.

```toml
[registry.main]
url = "http://registry.example.org/unio"
allow_unsigned = false      # see section 7

[limits]
archive_bytes = 67108864    # the largest archive
unpacked_bytes = 67108864   # the files of one package together
file_bytes = 16777216       # one file
entries = 10000             # entries of one archive
depth = 32                  # directories in a name
```

The download worker may reach only the hosts of the configured registries (SB-12): an archive URL on another host is refused before anything is downloaded, and so is a package that must be downloaded from a registry the configuration does not list.

## 6. The review of T-9

Installing a package that has modules marked `unsafe module` (listed in `unsafe_modules` or found by parsing its modules), or that declares permissions the root package does not have, needs an explicit confirmation. The tool shows, for each such package, the marked modules, that in 0.1 they contain no external functions (E0086 rejects them) and receive no wider permissions (RFC 0004), and the permissions it declares beyond the root's own; a module that carries the marker but is not listed, or is listed but does not carry it, is a warning. The confirmation is `--unsafe-ok`, or `yes` typed on a terminal; without it nothing is changed. `unio install` asks for every such package of the lock; `add` and `update` for the packages that are new or changed.

The confirmation grants nothing. The permissions of a dependency never widen the root package's: a program runs with the permissions of the manifest of its own package narrowed by the policy (SB-2), and the dependency's `[permissions]` are not read for the sandbox. Statically, a call from the root package into a dependency carries the effects of the dependency, so the root's manifest must permit them (E0085), while the modules of the dependency are checked against its own manifest.

## 7. A registry served as static files

A registry is a directory tree that any static HTTP server can serve:

```text
<registry>/index/<owner>/<name>.json
<registry>/archives/<owner>/<name>/<version>.tar
<registry>/archives/<owner>/<name>/<version>.tar.sig
```

The index of a package:

```json
{
  "format": "unio-registry-index-1",
  "package": "acme/json",
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

Paths are relative to the registry and may not leave it (`..`, absolute paths, and other hosts are refused); dependencies name packages of the same registry. The manifest in the archive must have the name and version of the index entry and the same dependencies, or the package is refused. The client sends `GET` with `Connection: close`, follows no redirect, and limits the size of every response.

Signatures: the unpack worker calls the signature check after the hash of the archive matches and before it unpacks anything. Verification is not implemented yet (T-8, T-8a: the post-quantum envelope, trust anchors, rotation, revocation, and rollback protection are the work of the registry task). Until then a package of a registry is refused, signed or not, unless the operator set `allow_unsigned = true` for that registry; it is then installed with a warning that it was not verified. A present signature is never treated as verified. Path and archive dependencies are not registry packages: the root manifest names their exact content (the archive hash).

Only `http://` is supported: HTTPS needs a TLS implementation, which the compiler does not have (it adds no dependency for it yet; [compiler/README.md](../compiler/README.md)). The content of a package is bound by its hash in the lock and, once verification exists, by its signature, never by the transport.

## 8. The archive format

A package archive is an uncompressed POSIX ustar archive, as `tar --format=ustar -cf pkg.tar -C pkg .` makes it; the files may also be below one top-level directory. It is a standard that existing tools write, its fixed 512-byte headers are parsed with the Rust standard library alone, and an uncompressed archive cannot expand: every byte of every file is present in the archive, so the unpacked size never exceeds the archive size, which the limits bound before anything is read, and no decompression-ratio attack exists. Compression is not required by the specification; a compressed format would be a new format version with its own ratio limit.

Accepted are regular files and directories with portable relative names: ASCII letters, digits, `.`, `_`, `-`, `+`. Refused, by the unpack worker and before anything is written: absolute names, `..`, backslashes and other characters, names Windows reserves or cannot keep (`CON`, `aux.unio`, a trailing dot), symbolic and hard links, character and block devices, FIFOs, pax extended headers and GNU long names, duplicate names, names that differ only in case (they collide on case-insensitive file systems, and a package must unpack the same everywhere), a file where a directory is needed, more entries than `entries`, deeper names than `depth`, files and packages over the size limits, a size that runs past the end of the archive, a bad header checksum, and data after the end of the archive. Files are written new, without execute or special bits. A path dependency is copied with the same rules (a link in it is refused), leaving out names that start with a dot and `target/` and `unio.lock` at its top.

## 9. What is not done

- Signature verification and the registry service with signed publishing (T-8, T-8a): the next task. The hook above refuses registry packages until then, unless the operator accepts them unverified.
- HTTPS (no TLS implementation; see section 7).
- Target and runtime requirements in manifests (language-contracts section 6 asks installation to validate them); 0.1 manifests declare none.
- `unio publish`, yanking, and removing old cache entries.
- On Windows the workers have no restriction of files and the network yet (SB-4), so the download worker connects itself and the tools need the weaker sandbox there ([runtime.md](runtime.md) 9.7).
