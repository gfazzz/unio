//! `unio build`: native code for a checked program (docs/runtime.md 8.4, requirements C-5,
//! section 10). The program is lowered as for the interpreter, every lowered body becomes a
//! function of an object file made by Cranelift, and the system C compiler links the object
//! with `libunio.a`, the runtime built from this crate, into one executable.

pub mod codegen;
pub mod image;
pub(crate) mod support;
pub(crate) mod units;

use std::path::{Path, PathBuf};

/// Why a build stopped.
pub enum BuildError {
    /// The program does not compile: the analyses with errors, as `unio check` reports them.
    Program(Vec<crate::Analysis>),
    /// Anything else: a construct not compiled natively yet, a missing tool or runtime.
    Message(String),
}

/// Whether this compiler can make executables for the machine it runs on.
pub fn supported() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// Builds the program of `file` into the executable `out` (C-5), in this process. `unio build`
/// does the same in two sandboxed steps instead: [`compile_object`] in the worker of the tool,
/// and the C compiler, run as [`link_command`] says, in a link step (docs/runtime.md 9.7).
pub fn build(file: &Path, out: &Path, release: bool) -> Result<(), BuildError> {
    let object = compile_object(file, release)?;
    let runtime = runtime_library().map_err(BuildError::Message)?;
    link(&object, &runtime, out).map_err(BuildError::Message)
}

/// The object file of the program of `file`: everything of a build but linking.
pub fn compile_object(file: &Path, release: bool) -> Result<Vec<u8>, BuildError> {
    if !supported() {
        return Err(BuildError::Message(
            "native executables are made for Linux on x86-64 only so far".into(),
        ));
    }
    let manifest = crate::sandbox_cli::manifest_of(file).map_err(BuildError::Message)?;
    let file = file.to_path_buf();
    crate::on_front_end_stack(move || compile(&file, manifest, release))
}

/// The object file of the program: loads and checks it, lowers every body, embeds the image.
fn compile(
    file: &Path,
    manifest: Option<(String, PathBuf)>,
    release: bool,
) -> Result<Vec<u8>, BuildError> {
    let program = crate::load_for_run(&[file.to_path_buf()], crate::effects::Profile::Application)
        .map_err(BuildError::Program)?;
    let module = program.modules.iter().position(|m| m.report).unwrap_or(0);
    let main = program.modules[module].path.clone();
    let stdlib = program.stdlib_root().map(Path::to_path_buf);
    let mut files: Vec<(PathBuf, String)> = program
        .modules
        .iter()
        .map(|m| (m.path.clone(), m.analysis.source.clone()))
        .collect();
    // The manifests that decide the packages, and so the module names in traces.
    for m in program.modules.iter().filter(|m| !m.stdlib) {
        let toml = m.package.join("unio.toml");
        if let Ok(text) = std::fs::read_to_string(&toml) {
            files.push((toml, text));
        }
    }
    files.sort();
    files.dedup_by(|a, b| a.0 == b.0);
    let image = image::Embedded {
        main,
        stdlib,
        manifest,
        files,
    };
    let interp = crate::interp::Interp::new(program);
    let units = units::collect(&interp);
    codegen::compile(
        &interp,
        &units,
        &image.encode(),
        &codegen::Options { release },
    )
    .map_err(BuildError::Message)
}

/// `libunio.a`: `UNIO_RUNTIME_LIB`, or the one built next to this compiler.
pub fn runtime_library() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("UNIO_RUNTIME_LIB") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().map_err(|e| format!("the compiler's location: {e}"))?;
    let candidate = exe
        .parent()
        .map(|d| d.join("libunio.a"))
        .ok_or("the compiler's location")?;
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Test binaries live in target/<profile>/deps.
    if let Some(up) = exe
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join("libunio.a"))
    {
        if up.is_file() {
            return Ok(up);
        }
    }
    Err(format!(
        "the runtime library {} is missing; build it with cargo build -p unio",
        candidate.display()
    ))
}

/// The C compiler (`cc`, or `CC`) and its arguments that link the object file with the runtime
/// into the executable `out`.
pub fn link_command(
    object: &Path,
    runtime: &Path,
    out: &Path,
) -> (String, Vec<std::ffi::OsString>) {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let mut args: Vec<std::ffi::OsString> =
        vec!["-o".into(), out.into(), object.into(), runtime.into()];
    args.extend(
        [
            "-Wl,--gc-sections",
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]
        .map(std::ffi::OsString::from),
    );
    (cc, args)
}

/// Links the object with the runtime by the system C compiler (`cc`, or `CC`).
fn link(object: &[u8], runtime: &Path, out: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!(
        "unio-build-{}-{}",
        std::process::id(),
        out.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let obj = dir.join("program.o");
    std::fs::write(&obj, object).map_err(|e| format!("{}: {e}", obj.display()))?;
    let (cc, args) = link_command(&obj, runtime, out);
    let output = std::process::Command::new(&cc).args(&args).output();
    let _ = std::fs::remove_dir_all(&dir);
    let output = output.map_err(|e| format!("the C compiler {cc} cannot be run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "linking failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
