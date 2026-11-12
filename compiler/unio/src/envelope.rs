//! Signed envelopes and keys (docs/cryptography.md, sections 2 and 4; T-8, T-8a, R-6a): the one
//! signature format of Unio 0.1, for package archives, registry indexes, key endorsements,
//! revocation lists, and release manifests.
//!
//! The algorithm is ML-DSA-65 of FIPS 204, the pure (not pre-hashed) variant, with the context
//! string `unio-envelope-1`, through the pinned crate `ml-dsa` (compiler/README.md). No other
//! algorithm is accepted: an envelope that names another algorithm or parameter set is refused,
//! never verified some other way (no silent downgrade). Signing is the deterministic variant of
//! FIPS 204, so that signing needs no randomness; key generation takes 32 bytes from the
//! system's random source, in the `unio` tool.
//!
//! An envelope is ASCII text of nine lines, in this order and nothing else:
//!
//! ```text
//! unio-envelope-1
//! purpose package-archive
//! algorithm ml-dsa-65
//! parameters fips204-ml-dsa-65
//! key sha384:<96 hexadecimal digits: the identity of the signing key>
//! subject acme/json@1.2.0
//! sequence 0
//! digest sha384:<96 hexadecimal digits: the SHA-384 of the payload>
//! signature <6618 hexadecimal digits: 3309 bytes>
//! ```
//!
//! The signature covers the bytes of the first eight lines, newlines included. It binds the
//! purpose, the format version, the algorithm and parameter identifiers, the key identity, the
//! subject (what the payload is: a package and version, an index, a key), a sequence number for
//! rollback protection (0 where the purpose has none), and the SHA-384 of the complete payload.
//! The identity of a key is `sha384:` and the SHA-384 of `unio-public-key-1\nml-dsa-65\n`
//! followed by its encoded public key. Parsing is strict and bounded (`MAX_ENVELOPE`), and the
//! payload is never read before its envelope is verified.

use ml_dsa::{EncodedSignature, EncodedVerifyingKey, Keypair, MlDsa65, Signature, VerifyingKey};

pub const FORMAT: &str = "unio-envelope-1";
pub const ALGORITHM: &str = "ml-dsa-65";
pub const PARAMETERS: &str = "fips204-ml-dsa-65";
/// The FIPS 204 context string of every Unio signature.
pub const CONTEXT: &[u8] = b"unio-envelope-1";
pub const PUBLIC_KEY_BYTES: usize = 1952;
pub const SIGNATURE_BYTES: usize = 3309;
pub const SEED_BYTES: usize = 32;
/// The largest envelope accepted.
pub const MAX_ENVELOPE: usize = 8 * 1024;
const PUBLIC_FORMAT: &str = "unio-public-key-1";
const SECRET_FORMAT: &str = "unio-secret-key-1";

/// What an envelope may sign.
pub const PURPOSES: &[&str] = &[
    "package-archive",
    "registry-index",
    "key-endorsement",
    "revocations",
    "release-manifest",
];

// ---------------------------------------------------------------- hexadecimal

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(b & 15)]));
    }
    out
}

/// Lower-case hexadecimal of exactly `len` bytes.
pub fn unhex(text: &str, len: usize) -> Option<Vec<u8>> {
    if text.len() != len * 2 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks(2)
        .map(|p| Some(digit(p[0])? << 4 | digit(p[1])?))
        .collect()
}

/// `sha384:<hex>` of the bytes.
pub fn digest(bytes: &[u8]) -> String {
    format!("sha384:{}", crate::sha::sha384_hex(bytes))
}

fn valid_digest(text: &str) -> bool {
    text.strip_prefix("sha384:")
        .is_some_and(|h| unhex(h, 48).is_some())
}

/// A subject: printable ASCII without spaces, at most 200 characters.
pub fn valid_subject(text: &str) -> bool {
    !text.is_empty() && text.len() <= 200 && text.bytes().all(|b| b.is_ascii_graphic())
}

// ---------------------------------------------------------------- keys

/// An ML-DSA-65 public key with its identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub bytes: Vec<u8>,
    pub id: String,
}

/// The identity of an encoded public key.
pub fn key_id(public: &[u8]) -> String {
    let mut data = format!("{PUBLIC_FORMAT}\n{ALGORITHM}\n").into_bytes();
    data.extend_from_slice(public);
    digest(&data)
}

impl PublicKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<PublicKey, String> {
        if bytes.len() != PUBLIC_KEY_BYTES {
            return Err(format!(
                "an ML-DSA-65 public key has {PUBLIC_KEY_BYTES} bytes, not {}",
                bytes.len()
            ));
        }
        Ok(PublicKey {
            bytes: bytes.to_vec(),
            id: key_id(bytes),
        })
    }

    /// The file of a public key: four lines, the format, the algorithm, the identity, and
    /// the key in hexadecimal.
    pub fn to_file(&self) -> String {
        format!(
            "{PUBLIC_FORMAT}\nalgorithm {ALGORITHM}\nkey {}\npublic {}\n",
            self.id,
            hex(&self.bytes)
        )
    }

    pub fn parse_file(text: &[u8]) -> Result<PublicKey, String> {
        let lines = lines(text, 4 * 1024 + 256, 4)?;
        let [format, algorithm, key, public] = lines.as_slice() else {
            return Err("a public key file has four lines".into());
        };
        if *format != PUBLIC_FORMAT {
            return Err(format!(
                "not a public key file of the format {PUBLIC_FORMAT}"
            ));
        }
        known_algorithm(field(algorithm, "algorithm")?)?;
        let id = field(key, "key")?;
        let bytes = unhex(field(public, "public")?, PUBLIC_KEY_BYTES)
            .ok_or("the public key is not 1952 bytes in lower-case hexadecimal")?;
        let key = PublicKey::from_bytes(&bytes)?;
        if key.id != id {
            return Err(format!(
                "the public key file names the identity {id}, but its key is {}",
                key.id
            ));
        }
        Ok(key)
    }

    /// Whether `signature` is a valid ML-DSA-65 signature of `message` under this key, with the
    /// context of Unio.
    pub fn verifies(&self, message: &[u8], signature: &[u8]) -> bool {
        ml_dsa_verify(&self.bytes, message, CONTEXT, signature)
    }
}

// ---------------------------------------------------------------- the primitive

/// ML-DSA-65 `Verify` (FIPS 204, Algorithm 3) of an encoded public key and signature; a key or
/// signature of the wrong length or encoding does not verify. The known-answer and
/// interoperability tests (`tests/signatures.rs`) check exactly this function.
pub fn ml_dsa_verify(public: &[u8], message: &[u8], context: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(sig)) = (
        EncodedVerifyingKey::<MlDsa65>::try_from(public),
        EncodedSignature::<MlDsa65>::try_from(signature),
    ) else {
        return false;
    };
    let key = VerifyingKey::<MlDsa65>::decode(&key);
    let Some(sig) = Signature::<MlDsa65>::decode(&sig) else {
        return false;
    };
    key.verify_with_context(message, context, &sig)
}

fn signing_key(seed: &[u8; SEED_BYTES]) -> ml_dsa::SigningKey<MlDsa65> {
    ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml_dsa::Seed::from(*seed))
}

/// The encoded public key of the ML-DSA-65 key pair of a seed (`KeyGen_internal`).
pub fn ml_dsa_public(seed: &[u8; SEED_BYTES]) -> Vec<u8> {
    signing_key(seed).verifying_key().encode().to_vec()
}

/// The deterministic ML-DSA-65 signature (FIPS 204, Algorithm 2 with rnd = 0) of the message
/// with a context of at most 255 bytes, by the key pair of a seed.
pub fn ml_dsa_sign(seed: &[u8; SEED_BYTES], message: &[u8], context: &[u8]) -> Option<Vec<u8>> {
    let signature = signing_key(seed)
        .expanded_key()
        .sign_deterministic(message, context)
        .ok()?;
    Some(signature.encode().to_vec())
}

/// An ML-DSA-65 signing key, kept as its 32-byte seed (FIPS 204, `KeyGen_internal`). The seed
/// is overwritten with zeros when the key is dropped; the crate zeroizes its expanded form.
pub struct SecretKey {
    seed: [u8; SEED_BYTES],
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        wipe(&mut self.seed);
    }
}

/// Overwrites the bytes with zeros in a way the compiler keeps.
pub fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // SAFETY: a valid, aligned, exclusive reference to a byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({})", self.public().id)
    }
}

impl SecretKey {
    pub fn from_seed(seed: &[u8; SEED_BYTES]) -> SecretKey {
        SecretKey { seed: *seed }
    }

    pub fn public(&self) -> PublicKey {
        PublicKey::from_bytes(&ml_dsa_public(&self.seed)).expect("an encoded ML-DSA-65 key")
    }

    /// The deterministic ML-DSA-65 signature of `message` with the context of Unio.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        ml_dsa_sign(&self.seed, message, CONTEXT).expect("the context is shorter than 256 bytes")
    }

    /// The file of a secret key: the format, the algorithm, the identity of its public key,
    /// and the seed in hexadecimal. It is written only outside the project (`unio key new`).
    pub fn to_file(&self) -> String {
        format!(
            "{SECRET_FORMAT}\nalgorithm {ALGORITHM}\nkey {}\nseed {}\n",
            self.public().id,
            hex(&self.seed)
        )
    }

    pub fn parse_file(text: &[u8]) -> Result<SecretKey, String> {
        let lines = lines(text, 512, 4)?;
        let [format, algorithm, key, seed] = lines.as_slice() else {
            return Err("a secret key file has four lines".into());
        };
        if *format != SECRET_FORMAT {
            return Err(format!(
                "not a secret key file of the format {SECRET_FORMAT}"
            ));
        }
        known_algorithm(field(algorithm, "algorithm")?)?;
        let id = field(key, "key")?;
        let mut bytes = unhex(field(seed, "seed")?, SEED_BYTES)
            .ok_or("the seed is not 32 bytes in lower-case hexadecimal")?;
        let mut array = [0u8; SEED_BYTES];
        array.copy_from_slice(&bytes);
        wipe(&mut bytes);
        let secret = SecretKey { seed: array };
        wipe(&mut array);
        if secret.public().id != id {
            return Err(format!(
                "the secret key file names the identity {id}, but its seed gives {}",
                secret.public().id
            ));
        }
        Ok(secret)
    }

    /// Whether a file starts like a secret key file, so that it is never published.
    pub fn looks_like(text: &[u8]) -> bool {
        text.starts_with(SECRET_FORMAT.as_bytes())
    }
}

// ---------------------------------------------------------------- envelopes

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub purpose: String,
    pub key: String,
    pub subject: String,
    pub sequence: u64,
    /// `sha384:<hex>` of the payload.
    pub digest: String,
    pub signature: Vec<u8>,
    /// The bytes the signature covers.
    pub signed: Vec<u8>,
}

/// The lines of a small ASCII text, each ended by `\n`, at most `max` bytes and `count` lines.
fn lines(text: &[u8], max: usize, count: usize) -> Result<Vec<&str>, String> {
    if text.len() > max {
        return Err(format!("it is longer than {max} bytes"));
    }
    let text = std::str::from_utf8(text).map_err(|_| "it is not ASCII text".to_string())?;
    if !text.is_ascii() || text.contains('\r') {
        return Err("it is not ASCII text with \\n line ends".into());
    }
    let body = text
        .strip_suffix('\n')
        .ok_or("its last line does not end with a newline")?;
    let lines: Vec<&str> = body.split('\n').collect();
    if lines.len() != count {
        return Err(format!("it has {} lines, not {count}", lines.len()));
    }
    Ok(lines)
}

fn field<'a>(line: &'a str, name: &str) -> Result<&'a str, String> {
    line.strip_prefix(name)
        .and_then(|r| r.strip_prefix(' '))
        .filter(|v| !v.is_empty() && !v.contains(' '))
        .ok_or_else(|| format!("the line '{name} ...' is expected, not {:?}", shorten(line)))
}

fn shorten(line: &str) -> String {
    if line.len() > 60 {
        format!("{}...", &line[..60])
    } else {
        line.to_string()
    }
}

fn known_algorithm(name: &str) -> Result<(), String> {
    if name == ALGORITHM {
        Ok(())
    } else {
        Err(format!(
            "the signature algorithm {:?} is not accepted: Unio 0.1 accepts only {ALGORITHM} \
             (docs/cryptography.md), and never falls back to another",
            shorten(name)
        ))
    }
}

fn sequence(text: &str) -> Result<u64, String> {
    let n: u64 = text
        .parse()
        .map_err(|_| format!("the sequence {text:?} is not a whole number"))?;
    if n.to_string() != text {
        return Err(format!("the sequence {text:?} is not written canonically"));
    }
    Ok(n)
}

/// The eight signed lines of an envelope.
fn header(purpose: &str, key: &str, subject: &str, sequence: u64, digest: &str) -> String {
    format!(
        "{FORMAT}\npurpose {purpose}\nalgorithm {ALGORITHM}\nparameters {PARAMETERS}\nkey {key}\n\
         subject {subject}\nsequence {sequence}\ndigest {digest}\n"
    )
}

/// Signs the payload: the envelope text.
pub fn seal(
    key: &SecretKey,
    purpose: &str,
    subject: &str,
    sequence: u64,
    payload: &[u8],
) -> Result<String, String> {
    if !PURPOSES.contains(&purpose) {
        return Err(format!("{purpose:?} is not a purpose of an envelope"));
    }
    if !valid_subject(subject) {
        return Err(format!("{subject:?} is not a valid subject"));
    }
    let header = header(
        purpose,
        &key.public().id,
        subject,
        sequence,
        &digest(payload),
    );
    let signature = key.sign(header.as_bytes());
    Ok(format!("{header}signature {}\n", hex(&signature)))
}

impl Envelope {
    /// Parses an envelope, strictly; verifies nothing.
    pub fn parse(text: &[u8]) -> Result<Envelope, String> {
        let problem = |p: String| format!("the signature envelope is malformed: {p}");
        let lines = lines(text, MAX_ENVELOPE, 9).map_err(problem)?;
        let [format, purpose, algorithm, parameters, key, subject, seq, dig, signature] =
            lines.as_slice()
        else {
            unreachable!("nine lines")
        };
        if *format != FORMAT {
            return Err(format!(
                "the signature envelope has the unknown format {:?}; this unio reads {FORMAT}",
                shorten(format)
            ));
        }
        let purpose = field(purpose, "purpose").map_err(problem)?;
        if !PURPOSES.contains(&purpose) {
            return Err(format!(
                "the signature envelope has the unknown purpose {:?}",
                shorten(purpose)
            ));
        }
        known_algorithm(field(algorithm, "algorithm").map_err(problem)?)?;
        let parameters = field(parameters, "parameters").map_err(problem)?;
        if parameters != PARAMETERS {
            return Err(format!(
                "the signature parameters {:?} are not accepted: only {PARAMETERS}",
                shorten(parameters)
            ));
        }
        let key = field(key, "key").map_err(problem)?;
        if !valid_digest(key) {
            return Err(problem(
                "the key is not sha384:<96 hexadecimal digits>".into(),
            ));
        }
        let subject = field(subject, "subject").map_err(problem)?;
        if !valid_subject(subject) {
            return Err(problem("the subject is not valid".into()));
        }
        let sequence = sequence(field(seq, "sequence").map_err(problem)?).map_err(problem)?;
        let digest = field(dig, "digest").map_err(problem)?;
        if !valid_digest(digest) {
            return Err(problem(
                "the digest is not sha384:<96 hexadecimal digits>".into(),
            ));
        }
        let signature = unhex(
            field(signature, "signature").map_err(problem)?,
            SIGNATURE_BYTES,
        )
        .ok_or_else(|| {
            problem(format!(
                "the signature is not {SIGNATURE_BYTES} bytes in lower-case hexadecimal"
            ))
        })?;
        let signed_len = text.len() - lines[8].len() - 1;
        Ok(Envelope {
            purpose: purpose.into(),
            key: key.into(),
            subject: subject.into(),
            sequence,
            digest: digest.into(),
            signature,
            signed: text[..signed_len].to_vec(),
        })
    }

    /// Checks the signature with this key, which must be the key the envelope names.
    pub fn verify(&self, key: &PublicKey) -> Result<(), String> {
        if key.id != self.key {
            return Err(format!(
                "the envelope is signed by the key {}, not by {}",
                self.key, key.id
            ));
        }
        if !key.verifies(&self.signed, &self.signature) {
            return Err(format!(
                "the ML-DSA-65 signature of the key {} is not valid: the envelope or the \
                 signature was changed",
                self.key
            ));
        }
        Ok(())
    }

    /// Checks what the envelope says it signs.
    pub fn binds(&self, purpose: &str, subject: &str, payload: &[u8]) -> Result<(), String> {
        if self.purpose != purpose {
            return Err(format!(
                "the envelope signs a {}, not a {purpose}",
                self.purpose
            ));
        }
        if self.subject != subject {
            return Err(format!(
                "the envelope signs {}, not {subject}",
                self.subject
            ));
        }
        let actual = digest(payload);
        if self.digest != actual {
            return Err(format!(
                "the payload has the digest {actual}, but the envelope signs {}: it was changed",
                self.digest
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SecretKey {
        SecretKey::from_seed(&[n; 32])
    }

    #[test]
    fn an_envelope_verifies_and_binds() {
        let k = key(1);
        let text = seal(&k, "package-archive", "acme/json@1.0.0", 0, b"payload").unwrap();
        let e = Envelope::parse(text.as_bytes()).unwrap();
        e.verify(&k.public()).unwrap();
        e.binds("package-archive", "acme/json@1.0.0", b"payload")
            .unwrap();
        assert!(e
            .binds("registry-index", "acme/json@1.0.0", b"payload")
            .is_err());
        assert!(e
            .binds("package-archive", "acme/json@1.0.1", b"payload")
            .is_err());
        assert!(e
            .binds("package-archive", "acme/json@1.0.0", b"payloaD")
            .is_err());
        assert!(e.verify(&key(2).public()).is_err());
        // Deterministic signing: the same input gives the same envelope.
        assert_eq!(
            seal(&k, "package-archive", "acme/json@1.0.0", 0, b"payload").unwrap(),
            text
        );
    }

    #[test]
    fn every_changed_line_is_refused() {
        let k = key(3);
        let text = seal(&k, "registry-index", "acme/json", 7, b"index").unwrap();
        let public = k.public();
        let changes = [
            ("unio-envelope-1", "unio-envelope-2"),
            ("purpose registry-index", "purpose package-archive"),
            ("algorithm ml-dsa-65", "algorithm ml-dsa-44"),
            ("algorithm ml-dsa-65", "algorithm ed25519"),
            (
                "parameters fips204-ml-dsa-65",
                "parameters fips204-ml-dsa-87",
            ),
            ("subject acme/json", "subject acme/jsoN"),
            ("sequence 7", "sequence 8"),
            ("sequence 7", "sequence 07"),
        ];
        for (from, to) in changes {
            let changed = text.replacen(from, to, 1);
            let refused = Envelope::parse(changed.as_bytes()).and_then(|e| e.verify(&public));
            assert!(refused.is_err(), "{from} -> {to}");
        }
        // Every byte of the signed part and of the signature matters.
        let e = Envelope::parse(text.as_bytes()).unwrap();
        for at in [0, 17, e.signed.len() - 2] {
            let mut bytes = text.clone().into_bytes();
            bytes[at] ^= 1;
            let r = Envelope::parse(&bytes).and_then(|e| e.verify(&public));
            assert!(r.is_err(), "byte {at}");
        }
        let sig_at = e.signed.len() + "signature ".len();
        for at in [sig_at, sig_at + 3000, text.len() - 2] {
            let mut bytes = text.clone().into_bytes();
            bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
            let r = Envelope::parse(&bytes).and_then(|e| e.verify(&public));
            assert!(r.is_err(), "signature byte {at}");
        }
        // Truncated anywhere, extended, or with another line end.
        for cut in [0, 10, e.signed.len(), text.len() - 1] {
            assert!(
                Envelope::parse(&text.as_bytes()[..cut]).is_err(),
                "cut {cut}"
            );
        }
        assert!(Envelope::parse(format!("{text}x\n").as_bytes()).is_err());
        assert!(Envelope::parse(text.replace('\n', "\r\n").as_bytes()).is_err());
        let short_sig = text.replacen("signature ", "signature 00", 1);
        assert!(Envelope::parse(short_sig.as_bytes()).is_err());
    }

    #[test]
    fn key_files_round_trip_and_are_checked() {
        let k = key(4);
        let public = PublicKey::parse_file(k.public().to_file().as_bytes()).unwrap();
        assert_eq!(public, k.public());
        let again = SecretKey::parse_file(k.to_file().as_bytes()).unwrap();
        assert_eq!(again.public(), public);
        assert!(SecretKey::looks_like(k.to_file().as_bytes()));
        // A wrong identity, a truncated key, another algorithm.
        let other = key(5).public().id;
        let file = public.to_file();
        assert!(PublicKey::parse_file(file.replace(&public.id, &other).as_bytes()).is_err());
        let truncated = file.replacen("public ", "public 00", 1);
        assert!(PublicKey::parse_file(truncated.as_bytes()).is_err());
        let mut cut = file.clone();
        cut.truncate(file.len() - 3);
        cut.push('\n');
        assert!(PublicKey::parse_file(cut.as_bytes()).is_err());
        assert!(PublicKey::parse_file(file.replace("ml-dsa-65", "ml-dsa-87").as_bytes()).is_err());
        let secret = k.to_file();
        assert!(SecretKey::parse_file(secret.replace(&public.id, &other).as_bytes()).is_err());
        let seed_line = secret.lines().last().unwrap().to_string();
        let flipped = seed_line.replacen("seed 0", "seed 1", 1);
        assert!(SecretKey::parse_file(secret.replace(&seed_line, &flipped).as_bytes()).is_err());
    }
}
