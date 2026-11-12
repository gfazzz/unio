//! The signature check of a package from a registry (T-8, T-8a, SB-12): the hook the unpack
//! worker calls after the hash of the archive matches and before anything is unpacked.
//!
//! Verification is not implemented yet: the post-quantum signature envelope, its trust anchors,
//! rotation, revocation, and rollback protection are the work of the registry and signing task
//! (docs/cryptography.md, section 4; status gap 12). Until then the check fails closed: a
//! package of a registry is refused, signed or not, unless the operator set
//! `allow_unsigned = true` for that registry in the packages configuration, which installs it
//! unverified and says so. A signature that is present is never treated as verified.
//! Dependencies from a directory or from an archive with the hash in the manifest are not
//! registry packages: the root package's own manifest names their exact content.

/// What the unpack worker knows when it checks the signature of an archive.
pub struct Request<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub registry: &'a str,
    pub archive: &'a [u8],
    pub signature: Option<&'a [u8]>,
    pub allow_unsigned: bool,
}

/// The verdict: `Ok(note)` lets the unpacking go on, with a note for the person installing;
/// `Err` refuses the package.
pub fn check(request: &Request) -> Result<String, String> {
    let what = format!(
        "{} {} from {}",
        request.package, request.version, request.registry
    );
    // The place of the verifier: once it exists, a valid signature by a trusted key returns
    // Ok here, and every other case is refused, whatever `allow_unsigned` says about packages
    // without a signature.
    if request.allow_unsigned {
        return Ok(format!(
            "unio: warning: {what} is installed without signature verification: the operator \
             allowed unsigned packages for this registry, and verification is not implemented \
             yet (T-8)"
        ));
    }
    let signature = if request.signature.is_some() {
        "its signature cannot be verified yet"
    } else {
        "it has no signature"
    };
    Err(format!(
        "{what} is refused: {signature}, and this unio does not verify registry signatures yet \
         (T-8, docs/cryptography.md). The operator can accept the packages of this registry \
         unverified with allow_unsigned = true in the packages configuration"
    ))
}
