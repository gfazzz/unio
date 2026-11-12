//! The trust of a registry (T-8, T-8a; docs/cryptography.md, section 4): which keys may sign
//! its packages and indexes, and how that changes.
//!
//! - **The anchor** is a public key the operator provisions in the packages configuration
//!   (`anchor = "main.pub"` for the registry), independently of the registry: a key that only
//!   the registry names is never trusted.
//! - **Endorsements** (`<registry>/trust/keys`) add signing keys: each names a key, its public
//!   key, and an optional expiry date, signed (purpose `key-endorsement`) by the anchor or by a
//!   key that is itself trusted. Rotation is an endorsement of the new key by the old one or by
//!   the anchor. A chain is at most `MAX_CHAIN` endorsements long.
//! - **The revocation list** (`<registry>/trust/revocations`), signed by the anchor alone
//!   (purpose `revocations`), with a sequence number that never goes back: a revoked key, the
//!   keys endorsed only through it, and everything they signed are refused. The highest
//!   sequence a client has seen is its floor; an older list is a rollback and is refused.
//! - An endorsement whose expiry date has passed (UTC) makes its key untrusted from that day.
//!
//! Everything here runs in a worker of the tool (SB-12): the download worker for an index, the
//! unpack worker for an archive. The trust files are parsed strictly and bounded, and a file
//! whose signature does not hold is refused as a whole.

use std::collections::{BTreeMap, BTreeSet};

use crate::envelope::{self, Envelope, PublicKey, SecretKey};

const ENDORSEMENT_FORMAT: &str = "unio-key-endorsement-1";
const REVOCATIONS_FORMAT: &str = "unio-revocations-1";
const BUNDLE_FORMAT: &[u8] = b"unio-trust-bundle-1\n";
/// The longest chain of endorsements from the anchor to a signing key.
pub const MAX_CHAIN: usize = 8;
/// The largest file of endorsements and the largest revocation list.
pub const MAX_KEYS_FILE: u64 = 4 << 20;
pub const MAX_REVOCATIONS_FILE: u64 = 1 << 20;

/// What a worker needs to verify the signatures of one registry: the files as they were read,
/// and the lowest sequence of the revocation list it may accept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    pub anchor: Vec<u8>,
    pub keys: Vec<u8>,
    pub revocations: Vec<u8>,
    pub revocations_floor: u64,
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn take<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (len, rest) = data.split_first_chunk::<8>()?;
    let len = usize::try_from(u64::from_le_bytes(*len)).ok()?;
    if rest.len() < len {
        return None;
    }
    let (value, rest) = rest.split_at(len);
    *data = rest;
    Some(value)
}

/// Frames byte strings for a worker's input: each with its length.
pub fn frame(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        put(&mut out, p);
    }
    out
}

/// The byte strings of [`frame`], exactly `n` of them.
pub fn unframe(mut data: &[u8], n: usize) -> Option<Vec<&[u8]>> {
    let parts: Option<Vec<&[u8]>> = (0..n).map(|_| take(&mut data)).collect();
    parts.filter(|_| data.is_empty())
}

impl Bundle {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = BUNDLE_FORMAT.to_vec();
        put(&mut out, &self.anchor);
        put(&mut out, &self.keys);
        put(&mut out, &self.revocations);
        out.extend_from_slice(&self.revocations_floor.to_le_bytes());
        out
    }

    pub fn decode(data: &[u8]) -> Option<Bundle> {
        let mut data = data.strip_prefix(BUNDLE_FORMAT)?;
        let anchor = take(&mut data)?.to_vec();
        let keys = take(&mut data)?.to_vec();
        let revocations = take(&mut data)?.to_vec();
        let floor: [u8; 8] = data.try_into().ok()?;
        Some(Bundle {
            anchor,
            keys,
            revocations,
            revocations_floor: u64::from_le_bytes(floor),
        })
    }
}

/// A key endorsement: the payload that its envelope signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endorsement {
    pub key: PublicKey,
    /// `YYYY-MM-DD`, the first day on which the key is no longer trusted, or `None`.
    pub expires: Option<String>,
}

/// A date `YYYY-MM-DD`.
pub fn valid_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&text[5..7].parse::<u32>().unwrap_or(0))
        && (1..=31).contains(&text[8..10].parse::<u32>().unwrap_or(0))
}

/// Today's date in UTC, `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil(i64::try_from(secs / 86_400).unwrap_or(0))
}

/// The date of a day counted from 1970-01-01 (the algorithm of Howard Hinnant).
pub fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

impl Endorsement {
    pub fn payload(&self) -> String {
        format!(
            "{ENDORSEMENT_FORMAT}\nalgorithm {}\nkey {}\npublic {}\nexpires {}\n",
            envelope::ALGORITHM,
            self.key.id,
            envelope::hex(&self.key.bytes),
            self.expires.as_deref().unwrap_or("never")
        )
    }

    fn parse(lines: &[&str]) -> Result<Endorsement, String> {
        let [format, algorithm, key, public, expires] = lines else {
            return Err("an endorsement has five lines".into());
        };
        if *format != ENDORSEMENT_FORMAT {
            return Err(format!(
                "an endorsement is not of the format {ENDORSEMENT_FORMAT}"
            ));
        }
        let file = format!("unio-public-key-1\n{algorithm}\n{key}\n{public}\n");
        let key =
            PublicKey::parse_file(file.as_bytes()).map_err(|e| format!("an endorsement: {e}"))?;
        let expires = match expires.strip_prefix("expires ") {
            Some("never") => None,
            Some(date) if valid_date(date) => Some(date.to_string()),
            _ => return Err("an endorsement has no valid expires line".into()),
        };
        Ok(Endorsement { key, expires })
    }
}

/// One record of the file of endorsements: the payload and its envelope.
pub fn endorsement_record(signer: &SecretKey, endorsement: &Endorsement) -> Result<String, String> {
    let payload = endorsement.payload();
    let sealed = envelope::seal(
        signer,
        "key-endorsement",
        &endorsement.key.id,
        0,
        payload.as_bytes(),
    )?;
    Ok(format!("{payload}{sealed}"))
}

/// The records of a file of endorsements: (endorsement, its payload, its envelope).
pub fn parse_keys(text: &[u8]) -> Result<Vec<(Endorsement, Vec<u8>, Envelope)>, String> {
    if text.len() as u64 > MAX_KEYS_FILE {
        return Err(format!(
            "the trust file keys is larger than {MAX_KEYS_FILE} bytes"
        ));
    }
    let text = std::str::from_utf8(text).map_err(|_| "the trust file keys is not ASCII")?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("the trust file keys does not end with a newline".into());
    }
    let lines: Vec<&str> = text.lines().collect();
    if !lines.len().is_multiple_of(14) {
        return Err("the trust file keys is not a list of endorsements".into());
    }
    let mut out = Vec::new();
    for record in lines.chunks(14) {
        let endorsement =
            Endorsement::parse(&record[..5]).map_err(|e| format!("the trust file keys: {e}"))?;
        let mut sealed = record[5..].join("\n");
        sealed.push('\n');
        let envelope =
            Envelope::parse(sealed.as_bytes()).map_err(|e| format!("the trust file keys: {e}"))?;
        let mut payload = record[..5].join("\n");
        payload.push('\n');
        out.push((endorsement, payload.into_bytes(), envelope));
    }
    Ok(out)
}

/// The revocation list: its sequence and the revoked keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Revocations {
    pub sequence: u64,
    pub revoked: BTreeSet<String>,
}

impl Revocations {
    pub fn payload(&self) -> String {
        let mut out = format!("{REVOCATIONS_FORMAT}\nsequence {}\n", self.sequence);
        for id in &self.revoked {
            out.push_str(&format!("revoked {id}\n"));
        }
        out
    }

    /// The file: the payload and its envelope, signed by the anchor.
    pub fn file(&self, anchor: &SecretKey) -> Result<String, String> {
        let payload = self.payload();
        let sealed = envelope::seal(
            anchor,
            "revocations",
            "registry",
            self.sequence,
            payload.as_bytes(),
        )?;
        Ok(format!("{payload}{sealed}"))
    }

    /// The list and its envelope, parsed; nothing verified.
    pub fn parse_file(text: &[u8]) -> Result<(Revocations, Vec<u8>, Envelope), String> {
        let bad = |p: &str| format!("the trust file revocations {p}");
        if text.len() as u64 > MAX_REVOCATIONS_FILE {
            return Err(bad(&format!("is larger than {MAX_REVOCATIONS_FILE} bytes")));
        }
        let text = std::str::from_utf8(text).map_err(|_| bad("is not ASCII"))?;
        let at = text
            .find(&format!("\n{}\n", envelope::FORMAT))
            .ok_or_else(|| bad("has no signature envelope"))?;
        let (payload, sealed) = text.split_at(at + 1);
        let mut lines = payload.lines();
        if lines.next() != Some(REVOCATIONS_FORMAT) {
            return Err(bad(&format!("is not of the format {REVOCATIONS_FORMAT}")));
        }
        let sequence = lines
            .next()
            .and_then(|l| l.strip_prefix("sequence "))
            .and_then(|n| n.parse::<u64>().ok().filter(|v| v.to_string() == n))
            .ok_or_else(|| bad("has no valid sequence"))?;
        let mut revoked = BTreeSet::new();
        for line in lines {
            let id = line
                .strip_prefix("revoked ")
                .filter(|id| id.starts_with("sha384:") && envelope::unhex(&id[7..], 48).is_some())
                .ok_or_else(|| bad(&format!("has a malformed line {line:?}")))?;
            revoked.insert(id.to_string());
        }
        let list = Revocations { sequence, revoked };
        if list.payload() != payload {
            return Err(bad("is not written canonically (sorted, one key per line)"));
        }
        let envelope = Envelope::parse(sealed.as_bytes()).map_err(|e| bad(&e))?;
        Ok((list, payload.as_bytes().to_vec(), envelope))
    }
}

/// What a verified signature says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub key: String,
    pub sequence: u64,
    pub revocations: u64,
}

/// The keys a registry's signatures may come from, as the operator's anchor and the registry's
/// trust files establish them today.
#[derive(Debug)]
pub struct Trust {
    pub anchor: PublicKey,
    /// The trusted keys, with the length of their chain from the anchor (0 for the anchor).
    pub keys: BTreeMap<String, (PublicKey, usize)>,
    /// Keys that would be trusted but for their expiry date, for the message.
    pub expired: BTreeMap<String, String>,
    pub revoked: BTreeSet<String>,
    pub revocations: u64,
}

impl Trust {
    /// Verifies the trust files of a bundle against its anchor, on the given date.
    pub fn load(bundle: &Bundle, today: &str) -> Result<Trust, String> {
        let anchor = PublicKey::parse_file(&bundle.anchor)
            .map_err(|e| format!("the trust anchor of the operator: {e}"))?;
        let (list, payload, sealed) = Revocations::parse_file(&bundle.revocations)?;
        if sealed.key != anchor.id {
            return Err(format!(
                "the revocation list is signed by {}, not by the trust anchor {} of the \
                 operator; refused",
                sealed.key, anchor.id
            ));
        }
        sealed
            .verify(&anchor)
            .and_then(|()| sealed.binds("revocations", "registry", &payload))
            .map_err(|e| format!("the revocation list: {e}; refused"))?;
        if sealed.sequence != list.sequence {
            return Err("the revocation list and its envelope name two sequences".into());
        }
        if list.sequence < bundle.revocations_floor {
            return Err(format!(
                "the revocation list has the sequence {}, older than {} already seen: a \
                 rollback of the trust metadata is refused",
                list.sequence, bundle.revocations_floor
            ));
        }
        if list.revoked.contains(&anchor.id) {
            return Err(format!(
                "the trust anchor {} is in the revocation list of the registry: the operator \
                 must provision a new anchor",
                anchor.id
            ));
        }
        let mut trust = Trust {
            keys: BTreeMap::from([(anchor.id.clone(), (anchor.clone(), 0))]),
            anchor,
            expired: BTreeMap::new(),
            revoked: list.revoked,
            revocations: list.sequence,
        };
        let records = parse_keys(&bundle.keys)?;
        // Breadth first from the anchor: a key is trusted through the shortest chain of
        // endorsements by keys that are trusted, not revoked, and not expired.
        for depth in 1..=MAX_CHAIN {
            let mut added = Vec::new();
            for (endorsement, payload, sealed) in &records {
                let id = &endorsement.key.id;
                if trust.keys.contains_key(id) || trust.revoked.contains(id) {
                    continue;
                }
                let Some((signer, d)) = trust.keys.get(&sealed.key) else {
                    continue;
                };
                if *d != depth - 1 {
                    continue;
                }
                sealed
                    .verify(signer)
                    .and_then(|()| sealed.binds("key-endorsement", id, payload))
                    .map_err(|e| format!("the endorsement of the key {id}: {e}; refused"))?;
                if let Some(date) = &endorsement.expires {
                    if date.as_str() <= today {
                        trust.expired.insert(id.clone(), date.clone());
                        continue;
                    }
                }
                added.push((id.clone(), endorsement.key.clone()));
            }
            if added.is_empty() {
                break;
            }
            for (id, key) in added {
                trust.keys.entry(id).or_insert((key, depth));
            }
        }
        Ok(trust)
    }

    /// Verifies an envelope over a payload: a trusted key signed exactly this purpose, subject,
    /// and payload.
    pub fn verify(
        &self,
        sealed: &[u8],
        purpose: &str,
        subject: &str,
        payload: &[u8],
    ) -> Result<Verified, String> {
        let sealed = Envelope::parse(sealed)?;
        let Some((key, _)) = self.keys.get(&sealed.key) else {
            let why = if self.revoked.contains(&sealed.key) {
                "it is revoked".to_string()
            } else if let Some(date) = self.expired.get(&sealed.key) {
                format!("its endorsement expired on {date}")
            } else {
                format!(
                    "it is neither the trust anchor {} of the operator nor endorsed through a \
                     chain from it",
                    self.anchor.id
                )
            };
            return Err(format!(
                "signed by the key {}, which is not trusted: {why}",
                sealed.key
            ));
        };
        sealed.verify(key)?;
        sealed.binds(purpose, subject, payload)?;
        Ok(Verified {
            key: sealed.key.clone(),
            sequence: sealed.sequence,
            revocations: self.revocations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SecretKey {
        SecretKey::from_seed(&[n; 32])
    }

    fn bundle(anchor: &SecretKey, keys: &str, revoked: &[&SecretKey], seq: u64) -> Bundle {
        let list = Revocations {
            sequence: seq,
            revoked: revoked.iter().map(|k| k.public().id).collect(),
        };
        Bundle {
            anchor: anchor.public().to_file().into_bytes(),
            keys: keys.as_bytes().to_vec(),
            revocations: list.file(anchor).unwrap().into_bytes(),
            revocations_floor: 0,
        }
    }

    fn endorse(signer: &SecretKey, key: &SecretKey, expires: Option<&str>) -> String {
        endorsement_record(
            signer,
            &Endorsement {
                key: key.public(),
                expires: expires.map(str::to_string),
            },
        )
        .unwrap()
    }

    fn signs(trust: &Trust, key: &SecretKey) -> Result<Verified, String> {
        let sealed = envelope::seal(key, "package-archive", "a/b@1.0.0", 0, b"x").unwrap();
        trust.verify(sealed.as_bytes(), "package-archive", "a/b@1.0.0", b"x")
    }

    #[test]
    fn chains_rotation_revocation_and_expiry() {
        let (anchor, a, b, c) = (key(10), key(11), key(12), key(13));
        // a by the anchor, b by a (rotation by the old key), c by nobody trusted.
        let keys = endorse(&anchor, &a, None) + &endorse(&a, &b, None) + &endorse(&c, &c, None);
        let t = Trust::load(&bundle(&anchor, &keys, &[], 1), "2026-11-12").unwrap();
        assert!(signs(&t, &anchor).is_ok());
        assert_eq!(signs(&t, &a).unwrap().key, a.public().id);
        assert!(signs(&t, &b).is_ok());
        assert!(signs(&t, &c)
            .unwrap_err()
            .contains("neither the trust anchor"));
        // Revoking a revokes b, which only a endorsed, until the anchor endorses b itself.
        let t = Trust::load(&bundle(&anchor, &keys, &[&a], 2), "2026-11-12").unwrap();
        assert!(signs(&t, &a).unwrap_err().contains("revoked"));
        assert!(signs(&t, &b).is_err());
        let keys2 = keys.clone() + &endorse(&anchor, &b, None);
        let t = Trust::load(&bundle(&anchor, &keys2, &[&a], 2), "2026-11-12").unwrap();
        assert!(signs(&t, &b).is_ok());
        // Expiry: the day of expiry is no longer trusted.
        let keys3 = endorse(&anchor, &a, Some("2026-11-12"));
        let t = Trust::load(&bundle(&anchor, &keys3, &[], 1), "2026-11-12").unwrap();
        assert!(signs(&t, &a).unwrap_err().contains("expired on 2026-11-12"));
        let t = Trust::load(&bundle(&anchor, &keys3, &[], 1), "2026-11-11").unwrap();
        assert!(signs(&t, &a).is_ok());
    }

    #[test]
    fn tampered_or_rolled_back_trust_files_are_refused() {
        let (anchor, a, other) = (key(20), key(21), key(22));
        let keys = endorse(&anchor, &a, None);
        let good = bundle(&anchor, &keys, &[], 5);
        // A changed public key in an endorsement.
        let mut tampered = good.clone();
        tampered.keys = keys
            .replacen("expires never", "expires 2999-01-01", 1)
            .into_bytes();
        assert!(Trust::load(&tampered, "2026-11-12").is_err());
        // A revocation list signed by another key, or an older one than seen.
        let mut wrong = good.clone();
        wrong.revocations = Revocations::default().file(&other).unwrap().into_bytes();
        assert!(Trust::load(&wrong, "2026-11-12")
            .unwrap_err()
            .contains("not by the trust anchor"));
        let mut old = good.clone();
        old.revocations_floor = 6;
        assert!(Trust::load(&old, "2026-11-12")
            .unwrap_err()
            .contains("rollback"));
        let mut edited = good.clone();
        edited.revocations = String::from_utf8(good.revocations.clone())
            .unwrap()
            .replacen("sequence 5\n", "sequence 5\nrevoked sha384:00\n", 1)
            .into_bytes();
        assert!(Trust::load(&edited, "2026-11-12").is_err());
        // A truncated file of endorsements.
        let mut cut = good.clone();
        cut.keys.truncate(cut.keys.len() - 100);
        assert!(Trust::load(&cut, "2026-11-12").is_err());
        // The bundle round-trips through its encoding, and a cut one does not decode.
        assert_eq!(Bundle::decode(&good.encode()).unwrap(), good);
        let encoded = good.encode();
        assert!(Bundle::decode(&encoded[..encoded.len() - 1]).is_none());
    }

    #[test]
    fn dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_404), "2025-11-12");
        assert_eq!(civil(11_016), "2000-02-29");
        assert!(valid_date("2026-02-28") && !valid_date("2026-13-01") && !valid_date("26-1-1"));
    }
}
