//! The cryptography of Unio's signatures (docs/cryptography.md, section 5; T-8a, R-6c): the
//! known-answer vectors of NIST's ACVP for ML-DSA-65 (FIPS 204; `tests/data/fips204`, see its
//! SOURCE file), interoperability with an independent implementation (the crate `fips204`, a
//! dev-dependency only), negative cases of keys, signatures, and envelopes, SHA-384 against an
//! independent implementation, the measured sizes, and the signed manifest of a release
//! (`unio release sign` and `verify`).

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer, Verifier};
use ml_dsa::{ExpandedSigningKey, MlDsa65};
use unio::envelope::{self, Envelope, PublicKey, SecretKey};
use unio::lsp::json::Json;

fn unhex(text: &str) -> Vec<u8> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("not hexadecimal: {c}"),
    };
    text.as_bytes()
        .chunks(2)
        .map(|p| digit(p[0]) << 4 | digit(p[1]))
        .collect()
}

fn vectors(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/fips204")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let json = Json::parse(&text).unwrap();
    assert_eq!(json.get("algorithm").and_then(Json::as_str), Some("ML-DSA"));
    assert_eq!(json.get("revision").and_then(Json::as_str), Some("FIPS204"));
    json
}

fn groups(json: &Json) -> Vec<&Json> {
    let groups = json.get("testGroups").and_then(Json::as_array).unwrap();
    for g in groups {
        assert_eq!(
            g.get("parameterSet").and_then(Json::as_str),
            Some("ML-DSA-65")
        );
    }
    groups.iter().collect()
}

fn tests(group: &Json) -> &[Json] {
    group.get("tests").and_then(Json::as_array).unwrap()
}

fn field(test: &Json, name: &str) -> Vec<u8> {
    unhex(test.get(name).and_then(Json::as_str).unwrap_or(""))
}

fn seed(bytes: &[u8]) -> [u8; 32] {
    bytes.try_into().unwrap()
}

/// A small deterministic generator for the inputs of the tests.
struct Bytes(u64);

impl Bytes {
    fn next(&mut self, n: usize) -> Vec<u8> {
        (0..n)
            .map(|_| {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 as u8
            })
            .collect()
    }
}

// ---------------------------------------------------------------- known answers (ACVP)

#[test]
#[allow(deprecated)]
fn fips204_key_generation_known_answers() {
    let json = vectors("ml-dsa-65-keygen.json");
    let mut n = 0;
    for group in groups(&json) {
        for test in tests(group) {
            let xi = seed(&field(test, "seed"));
            // The function that makes the public key of every Unio key.
            assert_eq!(envelope::ml_dsa_public(&xi), field(test, "pk"), "{test:?}");
            let sk = ExpandedSigningKey::<MlDsa65>::from_seed(&ml_dsa::Seed::from(xi));
            assert_eq!(sk.to_expanded().as_slice(), field(test, "sk").as_slice());
            n += 1;
        }
    }
    assert_eq!(n, 25);
}

#[test]
#[allow(deprecated)]
fn fips204_signature_generation_known_answers() {
    let json = vectors("ml-dsa-65-siggen.json");
    let mut n = 0;
    for group in groups(&json) {
        assert_eq!(
            group.get("signatureInterface").and_then(Json::as_str),
            Some("external")
        );
        assert_eq!(group.get("preHash").and_then(Json::as_str), Some("pure"));
        let deterministic = group.get("deterministic").and_then(Json::as_bool).unwrap();
        for test in tests(group) {
            let sk = field(test, "sk");
            let sk =
                ExpandedSigningKey::<MlDsa65>::from_expanded(sk.as_slice().try_into().unwrap());
            let (message, context) = (field(test, "message"), field(test, "context"));
            let signature = if deterministic {
                sk.sign_deterministic(&message, &context).unwrap()
            } else {
                // ML-DSA.Sign with the given rnd: M' = 0 || |ctx| || ctx || M.
                let rnd = ml_dsa::B32::try_from(field(test, "rnd").as_slice()).unwrap();
                let prefix = [0u8, context.len() as u8];
                sk.sign_internal(&[&prefix, &context, &message], &rnd)
            };
            let expected = field(test, "signature");
            assert_eq!(
                signature.encode().as_slice(),
                expected.as_slice(),
                "{test:?}"
            );
            // The verification of Unio accepts it.
            let public = sk.verifying_key().encode();
            assert!(envelope::ml_dsa_verify(
                &public, &message, &context, &expected
            ));
            n += 1;
        }
    }
    assert_eq!(n, 30);
}

#[test]
fn fips204_signature_verification_known_answers() {
    let json = vectors("ml-dsa-65-sigver.json");
    let (mut valid, mut invalid) = (0, 0);
    for group in groups(&json) {
        for test in tests(group) {
            let expected = test.get("testPassed").and_then(Json::as_bool).unwrap();
            let verified = envelope::ml_dsa_verify(
                &field(test, "pk"),
                &field(test, "message"),
                &field(test, "context"),
                &field(test, "signature"),
            );
            assert_eq!(verified, expected, "{:?}", test.get("reason"));
            if expected {
                valid += 1;
            } else {
                invalid += 1;
            }
        }
    }
    assert!(valid > 0 && invalid > 0, "{valid} valid, {invalid} invalid");
}

// ---------------------------------------------------------------- an independent implementation

/// The crate `fips204` (a separate implementation, with its own SHA-3) and Unio's `ml-dsa` make
/// the same keys and the same deterministic signatures, and each verifies the other's.
#[test]
fn interoperability_with_an_independent_implementation() {
    let mut random = Bytes(0x9e37_79b9_7f4a_7c15);
    for round in 0..12 {
        let xi = seed(&random.next(32));
        let (pk, sk) = ml_dsa_65::KG::keygen_from_seed(&xi);
        let theirs = pk.clone().into_bytes();
        assert_eq!(
            envelope::ml_dsa_public(&xi),
            theirs.to_vec(),
            "round {round}"
        );
        let message = random.next(round * 97);
        let context = if round % 3 == 0 {
            envelope::CONTEXT.to_vec()
        } else {
            random.next(round * 21)
        };
        let ours = envelope::ml_dsa_sign(&xi, &message, &context).unwrap();
        let other = sk
            .try_sign_with_seed(&[0u8; 32], &message, &context)
            .unwrap();
        assert_eq!(ours, other.to_vec(), "round {round}");
        assert!(pk.verify(&message, &ours.clone().try_into().unwrap(), &context));
        // A randomized signature of the other implementation verifies in Unio.
        let randomized = sk
            .try_sign_with_seed(&seed(&random.next(32)), &message, &context)
            .unwrap();
        assert!(envelope::ml_dsa_verify(
            &theirs,
            &message,
            &context,
            &randomized
        ));
        assert!(!envelope::ml_dsa_verify(
            &theirs,
            &message,
            b"other",
            &randomized
        ));
    }
    // A whole envelope: sealed by Unio, verified by the other; and the other way round.
    let xi = [42u8; 32];
    let (pk, sk) = ml_dsa_65::KG::keygen_from_seed(&xi);
    let key = SecretKey::from_seed(&xi);
    let sealed = envelope::seal(&key, "package-archive", "acme/json@1.0.0", 0, b"archive").unwrap();
    let e = Envelope::parse(sealed.as_bytes()).unwrap();
    assert!(pk.verify(
        &e.signed,
        &e.signature.clone().try_into().unwrap(),
        envelope::CONTEXT
    ));
    let signature = sk
        .try_sign_with_seed(&[7u8; 32], &e.signed, envelope::CONTEXT)
        .unwrap();
    let theirs = format!(
        "{}signature {}\n",
        String::from_utf8(e.signed.clone()).unwrap(),
        envelope::hex(&signature)
    );
    let parsed = Envelope::parse(theirs.as_bytes()).unwrap();
    parsed.verify(&key.public()).unwrap();
    parsed
        .binds("package-archive", "acme/json@1.0.0", b"archive")
        .unwrap();
}

// ---------------------------------------------------------------- negative cases

#[test]
fn modified_and_truncated_keys_and_signatures_are_refused() {
    let xi = [5u8; 32];
    let public = envelope::ml_dsa_public(&xi);
    let message = b"the payload";
    let signature = envelope::ml_dsa_sign(&xi, message, envelope::CONTEXT).unwrap();
    assert!(envelope::ml_dsa_verify(
        &public,
        message,
        envelope::CONTEXT,
        &signature
    ));
    let refused =
        |pk: &[u8], sig: &[u8], msg: &[u8], ctx: &[u8]| !envelope::ml_dsa_verify(pk, msg, ctx, sig);
    // Keys: truncated, extended, each region changed.
    assert!(refused(
        &public[..public.len() - 1],
        &signature,
        message,
        envelope::CONTEXT
    ));
    assert!(refused(
        &[&public[..], &[0]].concat(),
        &signature,
        message,
        envelope::CONTEXT
    ));
    for at in [0, 31, 32, 1000, public.len() - 1] {
        let mut pk = public.clone();
        pk[at] ^= 0x01;
        assert!(
            refused(&pk, &signature, message, envelope::CONTEXT),
            "key byte {at}"
        );
    }
    // Signatures: empty, truncated, extended, each region changed (c~, z, hint).
    assert!(refused(&public, &[], message, envelope::CONTEXT));
    assert!(refused(
        &public,
        &signature[..signature.len() - 1],
        message,
        envelope::CONTEXT
    ));
    assert!(refused(
        &public,
        &[&signature[..], &[0]].concat(),
        message,
        envelope::CONTEXT
    ));
    for at in [0, 47, 48, 1500, 3200, signature.len() - 1] {
        let mut sig = signature.clone();
        sig[at] ^= 0x80;
        assert!(
            refused(&public, &sig, message, envelope::CONTEXT),
            "signature byte {at}"
        );
    }
    // Another message, another context, a context longer than FIPS 204 allows.
    assert!(refused(
        &public,
        &signature,
        b"the payloaD",
        envelope::CONTEXT
    ));
    assert!(refused(&public, &signature, message, b"unio-envelope-2"));
    assert!(refused(&public, &signature, message, &[0u8; 256]));
    assert!(envelope::ml_dsa_sign(&xi, message, &[0u8; 256]).is_none());
    // The key files of Unio refuse a key of the wrong length.
    assert!(PublicKey::from_bytes(&public[..100]).is_err());
}

/// The envelope binds every field, and a removed or corrupted post-quantum signature is never
/// accepted some other way (docs/cryptography.md, section 5, item 3).
#[test]
fn envelopes_refuse_downgrades_and_substitutions() {
    let key = SecretKey::from_seed(&[6; 32]);
    let other = SecretKey::from_seed(&[7; 32]);
    let sealed = envelope::seal(&key, "registry-index", "acme/json", 4, b"index").unwrap();
    let ok = |text: &str| -> Result<(), String> {
        let e = Envelope::parse(text.as_bytes())?;
        e.verify(&key.public())?;
        e.binds("registry-index", "acme/json", b"index")
    };
    ok(&sealed).unwrap();
    let signature_line = sealed.lines().last().unwrap();
    let cases = [
        (
            "no signature",
            sealed.replace(&format!("{signature_line}\n"), ""),
        ),
        (
            "an empty signature",
            sealed.replace(signature_line, "signature "),
        ),
        ("ML-DSA-44", sealed.replace("ml-dsa-65", "ml-dsa-44")),
        (
            "SLH-DSA",
            sealed.replace("algorithm ml-dsa-65", "algorithm slh-dsa-shake-192s"),
        ),
        (
            "Ed25519",
            sealed.replace("algorithm ml-dsa-65", "algorithm ed25519"),
        ),
        (
            "other parameters",
            sealed.replace("fips204-ml-dsa-65", "fips204-ml-dsa-65-prehash"),
        ),
        (
            "another format",
            sealed.replace("unio-envelope-1", "unio-envelope-0"),
        ),
        (
            "another purpose",
            sealed.replace("registry-index", "release-manifest"),
        ),
        ("another subject", sealed.replace("acme/json", "acme/jsonx")),
        (
            "an older sequence",
            sealed.replace("sequence 4", "sequence 3"),
        ),
        (
            "another key",
            sealed.replace(&key.public().id, &other.public().id),
        ),
        (
            "another digest",
            sealed.replace(&envelope::digest(b"index"), &envelope::digest(b"x")),
        ),
        (
            "the signature of another key",
            sealed.replace(
                signature_line,
                &format!(
                    "signature {}",
                    envelope::hex(
                        &other.sign(
                            Envelope::parse(sealed.as_bytes())
                                .unwrap()
                                .signed
                                .as_slice()
                        )
                    )
                ),
            ),
        ),
    ];
    for (name, text) in cases {
        assert!(ok(&text).is_err(), "{name} was accepted");
    }
    // A huge envelope is refused before it is read further.
    let huge = format!("{sealed}{}", "x".repeat(envelope::MAX_ENVELOPE));
    assert!(Envelope::parse(huge.as_bytes())
        .unwrap_err()
        .contains("longer than"));
}

// ---------------------------------------------------------------- SHA-384

/// The SHA-384 of the payload digests is Unio's own (`unio::sha`): it agrees with the examples
/// of FIPS 180-4 and with an independent implementation on every length around the blocks.
#[test]
fn sha384_agrees_with_fips_180_and_an_independent_implementation() {
    use sha2::Digest;
    assert_eq!(
        unio::sha::sha384_hex(b"abc"),
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        unio::sha::sha384_hex(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
        "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039"
    );
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        unio::sha::sha384_hex(&million),
        "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"
    );
    let mut random = Bytes(12345);
    for len in (0..400).chain([1023, 1024, 1025, 65_537]) {
        let data = random.next(len);
        assert_eq!(
            unio::sha::sha384(&data).to_vec(),
            sha2::Sha384::digest(&data).to_vec(),
            "length {len}"
        );
    }
}

// ---------------------------------------------------------------- sizes

/// The sizes the policy asks to measure (docs/cryptography.md, section 5, item 6), recorded in
/// the document: they must not change silently.
#[test]
fn the_sizes_of_keys_signatures_and_envelopes() {
    let key = SecretKey::from_seed(&[8; 32]);
    assert_eq!(key.public().bytes.len(), 1952);
    assert_eq!(key.public().to_file().len(), 4058);
    assert_eq!(key.to_file().len(), 216);
    let sealed = envelope::seal(&key, "package-archive", "acme/json@1.0.0", 0, b"x").unwrap();
    assert_eq!(
        Envelope::parse(sealed.as_bytes()).unwrap().signature.len(),
        3309
    );
    assert_eq!(sealed.len(), 6972);
    // The time of each operation, printed for docs/cryptography.md (run with --release and
    // --nocapture); not asserted, since machines differ.
    let n = 20u32;
    let started = std::time::Instant::now();
    for i in 0..n {
        let _ = envelope::ml_dsa_public(&[i as u8; 32]);
    }
    let keygen = started.elapsed() / n;
    let message = vec![7u8; 4096];
    let started = std::time::Instant::now();
    let mut signatures = Vec::new();
    for i in 0..n {
        signatures.push(envelope::ml_dsa_sign(&[8; 32], &message, &[i as u8]).unwrap());
    }
    let sign = started.elapsed() / n;
    let public = key.public();
    let started = std::time::Instant::now();
    for (i, sig) in signatures.iter().enumerate() {
        assert!(envelope::ml_dsa_verify(
            &public.bytes,
            &message,
            &[i as u8],
            sig
        ));
    }
    let verify = started.elapsed() / n;
    eprintln!("ML-DSA-65: key generation {keygen:?}, signing {sign:?}, verification {verify:?}");
}

// ---------------------------------------------------------------- release manifests

fn unio(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env("HOME", dir)
        .env_remove("UNIO_POLICY")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap()
}

fn all(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

#[test]
fn release_manifests_are_signed_and_verified() {
    let dir = std::env::temp_dir().join(format!("unio-release-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out/bin")).unwrap();
    std::fs::write(dir.join("out/bin/unio"), b"the compiler").unwrap();
    std::fs::write(dir.join("out/sbom.json"), b"{}").unwrap();
    let run = |args: &[&str]| unio(&dir, args);
    let ok = |o: Output| assert!(o.status.success(), "{}", all(&o));
    let refused = |o: Output, expected: &str| {
        assert!(!o.status.success(), "succeeded: {}", all(&o));
        assert!(
            all(&o).contains(expected),
            "expected {expected:?}: {}",
            all(&o)
        );
    };
    ok(run(&["key", "new", "release", "--dir", "keys"]));
    ok(run(&["key", "new", "other", "--dir", "keys"]));
    let sign = [
        "release",
        "sign",
        "--key",
        "keys/release.key",
        "--subject",
        "unio-0.1.0-linux-x86_64",
        "--sequence",
        "3",
        "--out",
        "out/RELEASE",
        "out/bin/unio",
        "out/sbom.json",
    ];
    ok(run(&sign));
    let manifest = std::fs::read_to_string(dir.join("out/RELEASE")).unwrap();
    assert!(
        manifest.starts_with("unio-release-manifest-1\nfile sha384:"),
        "{manifest}"
    );
    assert!(manifest.contains(" bin/unio\n") && manifest.contains(" sbom.json\n"));
    let verify = |extra: &[&str]| {
        let mut args = vec![
            "release",
            "verify",
            "out/RELEASE",
            "--public",
            "keys/release.pub",
            "--subject",
            "unio-0.1.0-linux-x86_64",
        ];
        args.extend_from_slice(extra);
        run(&args)
    };
    ok(verify(&[]));
    ok(verify(&["--at-least", "3"]));
    // A manifest in the working directory, as the release job of CI writes it.
    let out = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir.join("out"))
        .args([
            "release",
            "sign",
            "--key",
            "../keys/release.key",
            "--subject",
            "s",
            "--sequence",
            "1",
            "--out",
            "HERE",
            "bin/unio",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", all(&out));
    let out = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir.join("out"))
        .args([
            "release",
            "verify",
            "HERE",
            "--public",
            "../keys/release.pub",
            "--subject",
            "s",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", all(&out));
    refused(verify(&["--at-least", "4"]), "a rollback is refused");
    refused(
        run(&[
            "release",
            "verify",
            "out/RELEASE",
            "--public",
            "keys/other.pub",
            "--subject",
            "unio-0.1.0-linux-x86_64",
        ]),
        "not by",
    );
    refused(
        run(&[
            "release",
            "verify",
            "out/RELEASE",
            "--public",
            "keys/release.pub",
            "--subject",
            "unio-0.1.0-macos",
        ]),
        "not unio-0.1.0-macos",
    );
    std::fs::write(dir.join("out/bin/unio"), b"the compiler, changed").unwrap();
    refused(verify(&[]), "bin/unio does not have the digest");
    std::fs::write(dir.join("out/bin/unio"), b"the compiler").unwrap();
    std::fs::write(
        dir.join("out/RELEASE"),
        manifest.replace(" sbom.json\n", " sbom2.json\n"),
    )
    .unwrap();
    refused(verify(&[]), "was changed");
    let _ = std::fs::remove_dir_all(&dir);
}
