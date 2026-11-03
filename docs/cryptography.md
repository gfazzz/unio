# Cryptography policy

Version: 0.1 (draft). Initial policy: post-quantum protection by default, with explicit compatibility boundaries. Russian localization: [docs/ru/cryptography.md](ru/cryptography.md).

This policy refines [concept section 6](concept.md), requirements T-8a and R-6a to R-6c in [the 0.1 specification](requirements.md), and OS-7 in [UnioS](os.md). It is a design requirement, not a claim that cryptography is implemented or audited. It does not change language syntax or require quantum hardware.

## 1. Scope and threat model

Protection covers recorded traffic that could be decrypted later by a quantum adversary, and future forgery of identities, packages, releases, updates, and trust metadata. It applies to project-controlled cryptographic interfaces and delivery paths as they are introduced: distribution in 0.1, networking in 0.2, and the UnioS track. External Git hosting and CI infrastructure have their own security properties; this policy does not certify them.

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

The 0.1 distribution and package work must meet the signature requirements before publication; the 0.2 networking RFC must define strict and compatibility profiles; UnioS U-0 specifies trust provisioning and recovery, and U-3 verifies transport, administration, and update paths before public deployment. Acceptance failures remain blockers for the affected security claim, not grounds to disable verification.

## 6. Standards

- [NIST FIPS 203: ML-KEM](https://csrc.nist.gov/pubs/fips/203/final).
- [NIST FIPS 204: ML-DSA](https://csrc.nist.gov/pubs/fips/204/final).
- [NIST FIPS 205: SLH-DSA](https://csrc.nist.gov/pubs/fips/205/final).

Implementation reviews must check applicable errata and the current protocol specifications. Algorithm standardization alone does not establish implementation validation or protocol security.
