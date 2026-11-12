# Cryptography policy

Version: 0.1 (draft). Initial policy: post-quantum protection by default, with explicit compatibility boundaries. Russian localization: [docs/ru/cryptography.md](ru/cryptography.md).

This policy refines [concept section 6](concept.md), requirements T-8a and R-6a to R-6c in [the 0.1 specification](requirements.md), and OS-7 in [UniOS](os.md). It is a design requirement, not a claim that cryptography is implemented or audited. It does not change language syntax or require quantum hardware.

Implementation status: the signature policy of section 4 is implemented for packages and registries (signed publishing, verification before extraction, trust anchors, rotation, revocation, expiry, rollback protection) and for the signed manifest of a release, with ML-DSA-65; section 7 gives the inventory, the checks, and what is missing. No component claims post-quantum compliance: the independent security review, fuzzing, and the review of timing that section 5 asks for have not been done ([status.md](status.md), rows T-8, T-8a, R-6 to R-6c). Nothing of section 3 is implemented: Unio 0.1 has no TLS. The SHA-384 content hashes of `unio symbol` (C-11a) and of packages identify content for caches and locks; they authenticate nothing.

## 1. Scope and threat model

Protection covers recorded traffic that could be decrypted later by a quantum adversary, and future forgery of identities, packages, releases, updates, and trust metadata. It applies to project-controlled cryptographic interfaces and delivery paths as they are introduced: distribution in 0.1, networking in 0.2, and the UniOS track. External Git hosting and CI infrastructure have their own security properties; this policy does not certify them.

Post-quantum confidentiality and post-quantum authentication are separate properties. A connection is called fully post-quantum protected only when both hold, including the authentication chain to its configured trust anchor. TLS 1.3, SSH, HTTPS, a content hash, or a signed file alone establishes neither property.

## 2. Primitive baseline

| Purpose | Initial choice | Constraint |
|---|---|---|
| Establish session keys | ML-KEM-768 (FIPS 203), combined with a classical key agreement in a reviewed hybrid protocol profile | ML-KEM is a key-encapsulation mechanism, not a signature or bulk cipher; use the profile's combiner and transcript binding, never an invented concatenation scheme |
| Sign artifacts and trust metadata | ML-DSA-65 (FIPS 204) | The signed envelope binds purpose, format version, algorithm and parameter identifiers, key identity, and the complete payload or its approved digest |
| Independent signature alternative | SLH-DSA (FIPS 205) | A separately reviewed profile selects the exact parameter set and use case, such as an offline trust root; it is not an automatic fallback |
| Encrypt data and backups | AES-256-GCM, or a separately reviewed AEAD with a 256-bit key | Enforce per-key nonce uniqueness and usage limits; use a reviewed key-management and storage format |
| Security-sensitive digests and key derivation | SHA-384/SHA-512 or SHA3-384/SHA3-512, and the protocol's specified KDF | Signatures and KEMs keep their standardized internal hash choices; a hash is not authentication |

These are project baseline choices, not a claim that every TLS or SSH implementation supports them. FIPS 203–205 standardize primitives, not complete TLS, SSH, certificate, or package formats. A protocol RFC must select the exact binding, parameter sets, wire identifiers, implementations, and interoperability targets before deployment. A different parameter set or algorithm requires an explicit policy revision and review, not runtime substitution.

Non-security uses such as hash tables or build-cache indexing are distinguished from authenticated artifact digests. No security claim follows from the word "hash". Security-sensitive algorithm identifiers are versioned so migration does not silently reinterpret existing signatures or lock files.

## 3. Connections and identities

- Project-controlled TLS and SSH endpoints default to a profile requiring hybrid post-quantum key establishment and post-quantum authentication. Authentication includes server or host identity and client or user identity wherever that identity is authenticated. RSA, ECDSA, or Ed25519 alone cannot satisfy that requirement.
- Certificate chains, SSH host and user trust, pinned keys, enrollment, rotation, revocation, and recovery must retain a post-quantum authenticated path to an independently provisioned trust anchor. An initial key fetched only over a classically authenticated channel cannot bootstrap that claim by itself.
- Negotiation is authenticated and bound to the session. Missing support, stripped offers, unknown mandatory identifiers, or invalid required signatures fail the connection. An automatic retry using a weaker profile is forbidden.
- Public browsers, certificate authorities, ACME services, and existing SSH clients may lack the required bindings. Compatibility access must be a separate, explicitly enabled policy scoped to an endpoint and purpose. It reports the actual key-establishment and authentication algorithms in both human and machine-readable status, records the exception, and makes no full post-quantum claim. A hybrid exchange with classical certificates is reported as post-quantum key establishment with classical authentication.
- Compatibility access cannot authorize release, package, or OS-update installation. Those objects must pass the independent signature policy below regardless of transport. If the strict protocol profile is unavailable, the strict endpoint remains unavailable; compatibility is not evidence that its acceptance criteria passed.

## 4. Packages, releases, updates, and stored data

- Before extraction, loading, or installation, verify the post-quantum signature on the authenticated manifest and the digest of every referenced payload. Cover compiler distributions, packages, OS images and updates, and associated trust metadata. A classical signature may accompany it for migration, but cannot replace it. A profile requiring multiple signatures verifies all of them, not whichever succeeds first.
- Provision trusted root keys independently of the artifact they authenticate. Define key rotation, revocation, expiration, and recovery, including a compromised or lost signing key. Reject unauthorized old metadata and rollback using authenticated version state; an intentional rollback needs an explicit authorized recovery procedure.
- Keep signing keys outside build runners, with controlled offline storage or suitable hardware protection, backup, and an exercised recovery procedure. Protect secret keys against accidental logging, serialization, and unnecessary copies; define zeroization and resource-lifetime behavior before implementing key APIs.
- Encryption of files, secrets, or backups uses a reviewed symmetric construction and key lifecycle. Any public-key wrapping, recovery recipient, or distribution of encryption keys must also satisfy the post-quantum policy. Password protection needs a separately reviewed password KDF and parameters; a 256-bit cipher does not compensate for a weak password.

## 5. Implementation and acceptance

Use standardized algorithms and vetted implementations. The bootstrap compiler and release tooling may temporarily use reviewed Rust libraries or external signing tools under concept section 1.1, with the dependency, reason, and replacement milestone recorded. This does not require a public `std.crypto` API in 0.1. A later Unio implementation must independently pass the same checks and external review before replacing that implementation. Do not invent cryptographic primitives to satisfy the self-hosting goal.

Before a component claims compliance, its release evidence includes:

1. An inventory of cryptographic uses, exact algorithms, parameter sets, protocol and format revisions, trust anchors, and explicitly scoped compatibility exceptions.
2. Standard known-answer tests and interoperability with an independent implementation; negative tests for modified and truncated keys, ciphertexts, signatures, and envelopes; bounded parsing and fuzzing of untrusted inputs.
3. Tests that removing or corrupting a required post-quantum component fails; no silent downgrade, classical-only signature acceptance, unauthorized rollback, or unverified trust bootstrap.
4. Rotation, revocation, expiration, backup, recovery, nonce management, and entropy-failure tests. Required randomness failing must stop the operation.
5. Review of secret-dependent timing and memory access, compiler effects, key handling, and protocol composition; independent security review before public exposure. Timing tests alone are not proof of constant-time behavior.
6. Measured key, signature, and message sizes; latency, memory, and denial-of-service limits on supported targets. Update resource estimates from measurements.

The 0.1 distribution and package work must meet the signature requirements before publication; the 0.2 networking RFC must define strict and compatibility profiles; UniOS U-0 specifies trust provisioning and recovery, and U-3 verifies transport, administration, and update paths before public deployment. Acceptance failures remain blockers for the affected security claim, not grounds to disable verification.

## 6. Standards

- [NIST FIPS 203: ML-KEM](https://csrc.nist.gov/pubs/fips/203/final).
- [NIST FIPS 204: ML-DSA](https://csrc.nist.gov/pubs/fips/204/final).
- [NIST FIPS 205: SLH-DSA](https://csrc.nist.gov/pubs/fips/205/final).

Implementation reviews must check applicable errata and the current protocol specifications. Algorithm standardization alone does not establish implementation validation or protocol security.

## 7. Implementation in release 0.1

This section records what is implemented and how it is checked (2026-11-12); it changes no requirement of sections 1 to 5. Formats and commands are in [packages.md](packages.md), section 7; the dependency is in [compiler/README.md](../compiler/README.md).

### 7.1 Inventory

| Use | Algorithm and parameters | Format | Code |
|---|---|---|---|
| Signatures of package archives, registry indexes, key endorsements, revocation lists, and release manifests | ML-DSA-65 (FIPS 204), the pure variant with the context string `unio-envelope-1`; deterministic signing (rnd = 0), so that signing needs no randomness | the envelope `unio-envelope-1` of nine lines, with the algorithm identifier `ml-dsa-65`, the parameter identifier `fips204-ml-dsa-65`, the purpose, the key identity, the subject, a sequence, and the SHA-384 of the whole payload | `compiler/unio/src/envelope.rs` |
| Digests of payloads and key identities | SHA-384 (FIPS 180-4), Unio's own implementation; a key identity is the SHA-384 of `unio-public-key-1\nml-dsa-65\n` and the encoded public key | `sha384:<96 hexadecimal digits>` | `compiler/unio/src/sha.rs` |
| Key generation | `ML-DSA.KeyGen_internal` from a 32-byte seed read from the system (`/dev/urandom` on Linux and macOS, `ProcessPrng` on Windows); a failure of the source stops the operation | the secret key file `unio-secret-key-1` holds the seed, readable by its owner only; the public key file `unio-public-key-1` | `compiler/unio/src/publish.rs` |
| Trust of a registry | the operator's anchor (`anchor` in the packages configuration), endorsements in `trust/keys` (at most 8 from the anchor, with an optional expiry date), the revocation list in `trust/revocations` signed by the anchor alone, with a sequence | [packages.md](packages.md), section 7.2 | `compiler/unio/src/package/trust.rs` |
| Rollback protection | signed sequences of each index and of each revocation list; floors in the cache (`trust/state`) and in the lock (`index_sequence`) | [packages.md](packages.md), section 7.3 | `compiler/unio/src/packages.rs` |
| Releases | the manifest `unio-release-manifest-1` (SHA-384 of every file) in an envelope with the purpose `release-manifest` and a sequence; the trust anchor is the operator's release key | `unio release sign`, `unio release verify` | `compiler/unio/src/release.rs` |

Compatibility exceptions: none. No classical signature is produced or accepted, and no option installs a registry package without a valid signature (the former `allow_unsigned` is refused). Transport: packages travel over plain HTTP; Unio 0.1 has no TLS, by a decision of 2026-11-12, and the privacy of downloads is not protected. Integrity, authenticity, and protection against substitution and rollback do not depend on the transport. TLS comes with the networking RFC of 0.2, which defines the profiles of section 3.

The implementation is the crate `ml-dsa` 0.1.2 of RustCrypto (MIT OR Apache-2.0), pinned to the exact version without default features and with `zeroize`, and the crates it needs: `crypto-common`, `hybrid-array`, `ctutils`, `cmov`, `typenum`, `module-lattice`, `num-traits`, `shake`, `digest`, `keccak`, `cfg-if`, `cpufeatures` (with `libc` on AArch64 Linux), `sponge-cursor`, `signature`, and `zeroize`. It is the reviewed library of the bootstrap compiler that section 5 allows: it is reviewed again when the compiler moves to its own tools, by 1.0, and a Unio implementation replaces it only after it passes the same checks and an external review.

### 7.2 Checks

- Standard vectors: the ACVP known-answer vectors of NIST for ML-DSA-65 (key generation, 25 cases; signature generation, 30 cases, deterministic and with a given rnd; signature verification, 15 valid and invalid cases), vendored in `compiler/unio/tests/data/fips204` with their source, their changes, and the NIST notice, run by `compiler/unio/tests/signatures.rs`.
- Interoperability with an independent implementation: the crate `fips204` 0.5.0 (MIT OR Apache-2.0, a dev-dependency only, with its own SHA-3) makes the same public keys and the same deterministic signatures for twelve seeds, messages, and contexts; each verifies the other's signatures, randomized ones included; and whole envelopes are sealed by one and verified by the other. SHA-384 agrees with the examples of FIPS 180-4 and with the crate `sha2` 0.10.9 (a dev-dependency) on every length from 0 to 399 bytes and around the block sizes.
- Negative tests: modified and truncated public keys and signatures, extended ones, another message or context, a context over 255 bytes; envelopes with every line changed, truncated, extended, with other line ends, with another algorithm (ML-DSA-44, ML-DSA-87, SLH-DSA, Ed25519), parameter set, format, purpose, subject, sequence, key, or digest, without a signature or with the signature of another key; key files with a wrong identity or length.
- Downgrade, tampering, and rollback, end to end over a static HTTP server on 127.0.0.1 (`compiler/unio/tests/packages.rs`): a changed archive, a changed archive with a changed index, a key nobody endorsed, a revoked key, an unknown algorithm of an archive or an index signature, a truncated or changed signature, a missing archive or index signature, the signature of another version or package, a changed endorsement, a revocation list not signed by the anchor, an older index (refused by the floor of the machine and by the lock), an older revocation list, a lock edited to another archive, a registry that removes a locked version, an unsigned index of format 1, and a registry without an anchor. Each is refused before anything of an archive is unpacked, and nothing is installed.
- Key lifecycle: rotation by the old key and by the anchor, a revocation that breaks the chain of the keys it endorsed, an expired endorsement, recovery after a compromised key (`unio registry resign`), the refusal of a revocation not signed by the anchor; keys never made inside a package or over an existing key; a package with a secret key file is not published.
- Measurements (one core of an Intel Xeon at 2.1 GHz, Linux x86-64, release build): key generation 0.27 ms, signing a 4 KiB message 0.96 ms, verification 0.16 ms. Sizes: public key 1,952 bytes (its file 4,058), signature 3,309 bytes, an envelope about 7 KB (6,972 bytes for the subject `acme/json@1.0.0`), secret key file 216 bytes. Limits on untrusted input: an envelope 8 KiB, the endorsements 4 MiB, a revocation list 1 MiB, an index 8 MiB, a chain 8 endorsements; one installation verifies each endorsement it uses once per worker.

These tests ran locally on Linux x86-64; in CI they run in the step "Tests" of the job `compiler` on Linux x86-64, macOS arm64, and Windows x86-64, which has not run with them yet. The release job of CI makes the SBOM (CycloneDX 1.5, `.github/scripts/sbom.py`) and signs and verifies a manifest of the release with a key made for that run only; the release key is the operator's, kept offline and never given to CI, and the real release signature is made outside CI after the comparison of reproducible builds.

### 7.3 Missing, which blocks the claim of section 5

- An independent security review, and a review of the secret-dependent timing and memory access of the implementation (the crate aims at constant time; Unio has not reviewed it).
- Fuzzing of the parsers of envelopes, keys, trust files, indexes, and release manifests; they are strict and bounded, and tested with the cases above.
- Backup and recovery of keys as an exercised procedure: a secret key file is the 32-byte seed, unencrypted on disk (no password KDF is selected, section 4); its backup is a copy kept offline. The recovery of a lost or compromised registry key is the procedure of [packages.md](packages.md), section 7.4, tested; the recovery of a lost anchor is a new anchor that every operator provisions again.
- A test of a failing random source by fault injection; the code stops on a failure, but no test forces one.
- Freshness: an index has no signed time, so withheld updates are not detected; one signature per object, no threshold; SLH-DSA is not selected.
- Secret keys in memory: the seed is overwritten when it is dropped and the crate zeroizes its expanded key, but no memory locking keeps them from swap.
