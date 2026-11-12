//! Keys, the registry side, and release signatures (T-8, T-8a, R-6, R-6a): `unio key`,
//! `unio registry`, `unio publish`, and `unio release` ([docs/packages.md](../../docs/packages.md)
//! sections 7 to 9).
//!
//! These commands read only what the operator or the publisher names on the command line (a
//! key file, a package directory, a registry directory, the files of a release) and write only
//! the registry directory, a new key, or a release manifest; they reach no network. Secret keys
//! are made from 32 bytes of the system's random source; if it fails, no key is made. A secret
//! key file is never written inside a package directory, and a package that contains one is not
//! published.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use unio::envelope::{self, Envelope, PublicKey, SecretKey};
use unio::package::registry;

type Res<T> = Result<T, String>;

/// Positional arguments and `--name value` options.
fn parse(
    arguments: &[String],
    with_values: &[&str],
) -> Res<(Vec<String>, BTreeMap<String, String>)> {
    let mut positional = Vec::new();
    let mut named = BTreeMap::new();
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        if let Some(name) = a.strip_prefix("--") {
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n.to_string(), v.to_string()),
                None => {
                    i += 1;
                    let value = arguments
                        .get(i)
                        .ok_or_else(|| format!("--{name} needs a value"))?;
                    (name.to_string(), value.clone())
                }
            };
            if !with_values.contains(&name.as_str()) {
                return Err(format!("unknown option '--{name}'"));
            }
            named.insert(name, value);
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    Ok((positional, named))
}

fn need<'a>(named: &'a BTreeMap<String, String>, name: &str) -> Res<&'a str> {
    named
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required"))
}

fn finish(command: &str, result: Res<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("unio {command}: error: {message}");
            ExitCode::from(1)
        }
    }
}

fn read_secret(path: &str) -> Res<SecretKey> {
    let mut bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let key = SecretKey::parse_file(&bytes).map_err(|e| format!("{path}: {e}"));
    envelope::wipe(&mut bytes);
    key
}

fn read_public(path: &str) -> Res<PublicKey> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    PublicKey::parse_file(&bytes).map_err(|e| format!("{path}: {e}"))
}

/// The package directory that holds `path`, if any: a directory with `unio.toml` at or above it.
fn package_above(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    absolute
        .ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .map(Path::to_path_buf)
}

/// 32 bytes of the system's random source. A failure stops the operation (docs/cryptography.md,
/// section 5, item 4): nothing falls back to a weaker source.
fn random_seed() -> Res<[u8; envelope::SEED_BYTES]> {
    let mut seed = [0u8; envelope::SEED_BYTES];
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut seed))
            .map_err(|e| format!("the system's random source /dev/urandom failed: {e}"))?;
    }
    #[cfg(windows)]
    {
        #[link(name = "bcryptprimitives", kind = "raw-dylib")]
        extern "system" {
            fn ProcessPrng(data: *mut u8, len: usize) -> i32;
        }
        // SAFETY: the buffer is valid and writable for its length.
        if unsafe { ProcessPrng(seed.as_mut_ptr(), seed.len()) } == 0 {
            return Err("the system's random source (ProcessPrng) failed".into());
        }
    }
    if seed.iter().all(|b| *b == 0) {
        return Err("the system's random source gave only zeros".into());
    }
    Ok(seed)
}

/// Writes a new file that only its owner may read, never over an existing one.
fn write_private(path: &Path, text: &str) -> Res<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------- unio key

/// `unio key new NAME [--dir DIR]`, `unio key id FILE`,
/// `unio key endorse PUBLIC --key SIGNER --registry DIR [--expires YYYY-MM-DD]`,
/// `unio key revoke PUBLIC|ID --key ANCHOR --registry DIR`.
pub fn key(arguments: &[String]) -> ExitCode {
    let result = (|| -> Res<()> {
        let (positional, named) = parse(arguments, &["dir", "key", "registry", "expires"])?;
        match positional
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["new", name] => key_new(name, named.get("dir").map(PathBuf::from)),
            ["id", file] => {
                let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
                let id = match PublicKey::parse_file(&bytes) {
                    Ok(k) => k.id,
                    Err(_) => read_secret(file)?.public().id,
                };
                println!("{id}");
                Ok(())
            }
            ["endorse", public] => {
                let key = read_public(public)?;
                let signer = read_secret(need(&named, "key")?)?;
                let expires = named.get("expires").cloned();
                if let Some(date) = &expires {
                    if !unio::package::trust::valid_date(date) {
                        return Err(format!("--expires {date}: give a date as YYYY-MM-DD"));
                    }
                }
                registry::endorse(Path::new(need(&named, "registry")?), &signer, &key, expires)?;
                eprintln!(
                    "unio: {} endorsed by {} in the registry's trust/keys",
                    key.id,
                    signer.public().id
                );
                Ok(())
            }
            ["revoke", which] => {
                let id = if which.starts_with("sha384:") {
                    which.to_string()
                } else {
                    read_public(which)?.id
                };
                let anchor = read_secret(need(&named, "key")?)?;
                let sequence =
                    registry::revoke(Path::new(need(&named, "registry")?), &anchor, &id)?;
                eprintln!("unio: {id} revoked; the revocation list has the sequence {sequence}");
                Ok(())
            }
            _ => Err(
                "usage: unio key new NAME [--dir DIR] | key id FILE | key endorse PUBLIC.pub \
                      --key SIGNER.key --registry DIR [--expires YYYY-MM-DD] | key revoke \
                      PUBLIC.pub|sha384:ID --key ANCHOR.key --registry DIR"
                    .into(),
            ),
        }
    })();
    finish("key", result)
}

fn key_new(name: &str, dir: Option<PathBuf>) -> Res<()> {
    if !unio::package::manifest::valid_name(name) {
        return Err(format!(
            "'{name}' is not a valid key name: lowercase letters, digits, '-' and '_'"
        ));
    }
    let dir = match dir {
        Some(dir) => dir,
        None => unio::package::config::config_dir()
            .ok_or("no configuration directory: give --dir DIR")?
            .join("keys"),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Some(package) = package_above(&dir) {
        return Err(format!(
            "{} is inside the package {}: a secret key is never kept in a project, where it \
             could be committed or published; choose a directory outside it",
            dir.display(),
            package.display()
        ));
    }
    let mut seed = random_seed()?;
    let secret = SecretKey::from_seed(&seed);
    envelope::wipe(&mut seed);
    let public = secret.public();
    let (secret_path, public_path) = (
        dir.join(format!("{name}.key")),
        dir.join(format!("{name}.pub")),
    );
    if public_path.exists() {
        return Err(format!("{} already exists", public_path.display()));
    }
    write_private(&secret_path, &secret.to_file())?;
    std::fs::write(&public_path, public.to_file())
        .map_err(|e| format!("{}: {e}", public_path.display()))?;
    eprintln!(
        "unio: a new ML-DSA-65 key {}\n  secret: {} (keep it offline; never in a project or CI)\n  \
         public: {}",
        public.id,
        secret_path.display(),
        public_path.display()
    );
    println!("{}", public.id);
    Ok(())
}

// ---------------------------------------------------------------- unio registry, publish

/// `unio registry init DIR --anchor ANCHOR.key`, `unio registry resign DIR --key KEY`.
pub fn registry_command(arguments: &[String]) -> ExitCode {
    let result = (|| -> Res<()> {
        let (positional, named) = parse(arguments, &["anchor", "key"])?;
        match positional
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["init", dir] => {
                let anchor = read_secret(need(&named, "anchor")?)?;
                registry::init(Path::new(dir), &anchor)?;
                eprintln!(
                    "unio: registry {dir} initialized with the trust anchor {}",
                    anchor.public().id
                );
                Ok(())
            }
            ["resign", dir] => {
                let key = read_secret(need(&named, "key")?)?;
                let n = registry::resign(Path::new(dir), &key)?;
                eprintln!(
                    "unio: {n} index(es) and their archives signed again by {}",
                    key.public().id
                );
                Ok(())
            }
            _ => Err(
                "usage: unio registry init DIR --anchor ANCHOR.key | registry resign DIR \
                      --key KEY.key"
                    .into(),
            ),
        }
    })();
    finish("registry", result)
}

/// `unio publish --registry DIR --owner OWNER --key KEY`, in a package directory.
pub fn publish(arguments: &[String]) -> ExitCode {
    let result = (|| -> Res<()> {
        let (positional, named) = parse(arguments, &["registry", "owner", "key"])?;
        if !positional.is_empty() {
            return Err("usage: unio publish --registry DIR --owner OWNER --key KEY.key".into());
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let package = package_above(&cwd).ok_or("no unio.toml in this directory or above it")?;
        let key_path = need(&named, "key")?;
        if package_above(Path::new(key_path)).is_some_and(|p| p == package) {
            return Err(format!(
                "the key {key_path} is inside the package {}: move it out of the package",
                package.display()
            ));
        }
        let key = read_secret(key_path)?;
        let published = registry::publish(
            Path::new(need(&named, "registry")?),
            need(&named, "owner")?,
            &package,
            &key,
        )?;
        if published.already {
            eprintln!(
                "unio: {} {} is already published with the same content; nothing changed",
                published.package, published.version
            );
        } else {
            eprintln!(
                "unio: published {} {} (sha384 {}), signed by {}; the index has the sequence {}",
                published.package,
                published.version,
                published.sha384,
                key.public().id,
                published.sequence
            );
        }
        Ok(())
    })();
    finish("publish", result)
}

// ---------------------------------------------------------------- unio release

/// `unio release sign --key KEY --subject NAME --sequence N --out MANIFEST FILE...` and
/// `unio release verify MANIFEST --public KEY.pub --subject NAME [--at-least N]`.
pub fn release(arguments: &[String]) -> ExitCode {
    let result = (|| -> Res<()> {
        let (positional, named) = parse(
            arguments,
            &["key", "subject", "sequence", "out", "public", "at-least"],
        )?;
        let Some((action, files)) = positional.split_first() else {
            return Err("usage: unio release sign ... | release verify ...".into());
        };
        match action.as_str() {
            "sign" => release_sign(files, &named),
            "verify" if files.len() == 1 => release_verify(&files[0], &named),
            _ => Err(
                "usage: unio release sign --key KEY.key --subject NAME --sequence N --out \
                      MANIFEST FILE... | release verify MANIFEST --public KEY.pub --subject NAME \
                      [--at-least N]"
                    .into(),
            ),
        }
    })();
    finish("release", result)
}

fn sequence(text: &str) -> Res<u64> {
    text.parse()
        .map_err(|_| format!("{text:?} is not a whole number"))
}

fn release_sign(files: &[String], named: &BTreeMap<String, String>) -> Res<()> {
    let key = read_secret(need(named, "key")?)?;
    let subject = need(named, "subject")?;
    let seq = sequence(need(named, "sequence")?)?;
    let out = PathBuf::from(need(named, "out")?);
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let base = std::path::absolute(parent.unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let base = std::fs::canonicalize(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let mut list = Vec::new();
    for file in files {
        let path = std::fs::canonicalize(file).map_err(|e| format!("{file}: {e}"))?;
        let name = path
            .strip_prefix(&base)
            .map_err(|_| format!("{file} is not below the directory of the manifest"))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(|e| format!("{file}: {e}"))?;
        list.push((name, bytes));
    }
    if list.is_empty() {
        return Err("no files to sign".into());
    }
    let manifest = unio::release::manifest(&list)?;
    let sealed = envelope::seal(&key, "release-manifest", subject, seq, manifest.as_bytes())?;
    std::fs::write(&out, &manifest).map_err(|e| format!("{}: {e}", out.display()))?;
    let sig = PathBuf::from(format!("{}.sig", out.display()));
    std::fs::write(&sig, sealed).map_err(|e| format!("{}: {e}", sig.display()))?;
    eprintln!(
        "unio: {} file(s) signed as {subject} (sequence {seq}) by {}: {} and {}",
        list.len(),
        key.public().id,
        out.display(),
        sig.display()
    );
    Ok(())
}

fn release_verify(manifest_path: &str, named: &BTreeMap<String, String>) -> Res<()> {
    let public = read_public(need(named, "public")?)?;
    let subject = need(named, "subject")?;
    let floor = named.get("at-least").map(|t| sequence(t)).transpose()?;
    let manifest = std::fs::read(manifest_path).map_err(|e| format!("{manifest_path}: {e}"))?;
    let sig_path = format!("{manifest_path}.sig");
    let sealed = std::fs::read(&sig_path).map_err(|e| format!("{sig_path}: {e}"))?;
    let sealed = Envelope::parse(&sealed)?;
    sealed.verify(&public)?;
    sealed.binds("release-manifest", subject, &manifest)?;
    if let Some(floor) = floor {
        if sealed.sequence < floor {
            return Err(format!(
                "the release has the sequence {}, older than {floor}: a rollback is refused",
                sealed.sequence
            ));
        }
    }
    let base = Path::new(manifest_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let entries = unio::release::parse(&manifest)?;
    for (name, digest) in &entries {
        let bytes = std::fs::read(base.join(name)).map_err(|e| format!("{name}: {e}"))?;
        if envelope::digest(&bytes) != *digest {
            return Err(format!(
                "{name} does not have the digest the signed manifest gives"
            ));
        }
    }
    eprintln!(
        "unio: {} file(s) of {subject} (sequence {}) verified: ML-DSA-65 signature of {}",
        entries.len(),
        sealed.sequence,
        public.id
    );
    Ok(())
}
