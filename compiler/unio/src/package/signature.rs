//! The signature check of a package from a registry (T-8, T-8a, SB-12), which the unpack worker
//! runs after the SHA-384 of the archive matches and before anything of the archive is read.
//!
//! The archive must carry an envelope (`<version>.tar.sig`; [`crate::envelope`]) with the
//! purpose `package-archive`, the subject `owner/name@version`, and the SHA-384 of the archive,
//! signed with ML-DSA-65 by a key that the operator's trust anchor establishes for the registry
//! today ([`super::trust`]). Anything else is refused: no signature, another algorithm, a key
//! that is not trusted, revoked, or expired, an envelope for another package or version, a
//! changed archive, or trust files that were changed or rolled back. There is no option to
//! accept a registry package unverified (docs/cryptography.md, section 3: no exception may
//! authorize package installation); a package whose exact content the root manifest names
//! (an archive dependency with its `sha384`, or a path) is not a registry package.

use super::trust::{Bundle, Trust, Verified};

/// What the unpack worker knows when it checks the signature of an archive.
pub struct Request<'a> {
    /// `owner/name`.
    pub package: &'a str,
    pub version: &'a str,
    /// The registry, for messages.
    pub registry: &'a str,
    pub archive: &'a [u8],
    pub signature: Option<&'a [u8]>,
    pub trust: &'a Bundle,
}

/// The verdict: `Ok` lets the unpacking go on; `Err` refuses the package.
pub fn check(request: &Request) -> Result<Verified, String> {
    let what = format!(
        "{} {} from {}",
        request.package, request.version, request.registry
    );
    let refused = |why: String| format!("{what} is refused before unpacking: {why}");
    let Some(signature) = request.signature else {
        return Err(refused(
            "it has no signature, and a registry package is installed only with a valid ML-DSA-65 \
             signature (T-8)"
                .into(),
        ));
    };
    let trust = Trust::load(request.trust, &super::trust::today()).map_err(&refused)?;
    trust
        .verify(
            signature,
            "package-archive",
            &format!("{}@{}", request.package, request.version),
            request.archive,
        )
        .map_err(refused)
}
