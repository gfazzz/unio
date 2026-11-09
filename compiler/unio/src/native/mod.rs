//! `unio build`: native code for a checked program (docs/runtime.md 8.4, requirements C-5,
//! section 10). The program is lowered as for the interpreter, every lowered body becomes a
//! function of an object file made by Cranelift, and the system C compiler links the object
//! with `libunio.a`, the runtime built from this crate, into one executable.

pub mod codegen;
pub mod image;
pub(crate) mod support;
pub mod toolchain;
pub(crate) mod units;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why a build stopped.
pub enum BuildError {
    /// The program does not compile: the analyses with errors, as `unio check` reports them.
    Program(Vec<crate::Analysis>),
    /// Anything else: a construct not compiled natively yet, a missing tool or runtime.
    Message(String),
}

/// A machine and system that `unio build` makes executables for (docs/runtime.md 8.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    LinuxX86_64,
    LinuxAarch64,
    MacosAarch64,
    WindowsX86_64,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::LinuxX86_64,
        Target::LinuxAarch64,
        Target::MacosAarch64,
        Target::WindowsX86_64,
    ];

    /// The target of the machine this compiler runs on, if executables are made for it: an
    /// executable is linked with the runtime built from this crate, so it is made for the
    /// machine and system of the compiler.
    pub fn host() -> Option<Target> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Target::LinuxX86_64)
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Some(Target::LinuxAarch64)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Target::MacosAarch64)
        } else if cfg!(all(windows, target_env = "msvc", target_arch = "x86_64")) {
            Some(Target::WindowsX86_64)
        } else {
            None
        }
    }

    /// The triple Cranelift generates code for. On macOS it names the oldest system the
    /// runtime supports (that of Rust's `aarch64-apple-darwin`), which the object file records.
    pub fn triple(self) -> &'static str {
        match self {
            Target::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Target::LinuxAarch64 => "aarch64-unknown-linux-gnu",
            Target::MacosAarch64 => "aarch64-apple-macosx11.0.0",
            Target::WindowsX86_64 => "x86_64-pc-windows-msvc",
        }
    }

    /// The name of the target in messages.
    pub fn name(self) -> &'static str {
        match self {
            Target::LinuxX86_64 => "Linux on x86-64",
            Target::LinuxAarch64 => "Linux on AArch64",
            Target::MacosAarch64 => "macOS on AArch64",
            Target::WindowsX86_64 => "Windows on x86-64",
        }
    }

    /// The file name of the runtime library next to the compiler.
    pub fn runtime_file(self) -> &'static str {
        match self {
            Target::WindowsX86_64 => "unio.lib",
            _ => "libunio.a",
        }
    }

    /// The suffix of an executable's file name.
    pub fn executable_suffix(self) -> &'static str {
        match self {
            Target::WindowsX86_64 => ".exe",
            _ => "",
        }
    }
}

/// Whether this compiler can make executables for the machine it runs on.
pub fn supported() -> bool {
    Target::host().is_some()
}

/// Why no executable is made here: the targets there are, and this machine.
pub fn unsupported() -> String {
    let names: Vec<&str> = Target::ALL.iter().map(|t| t.name()).collect();
    format!(
        "native executables are made for {}, and {}; this compiler runs on {} on {}",
        names[..names.len() - 1].join(", "),
        names[names.len() - 1],
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Builds the program of `file` into the executable `out` (C-5), in this process. `unio build`
/// does the same in two sandboxed steps instead: [`compile_object`] in the worker of the tool,
/// and the C compiler, run as [`link_command`] says, in a link step (docs/runtime.md 9.7).
pub fn build(file: &Path, out: &Path, release: bool) -> Result<(), BuildError> {
    let toolchain = toolchain::identity().map_err(BuildError::Message)?;
    let object = compile_object(file, release, &toolchain)?;
    let runtime = runtime_library().map_err(BuildError::Message)?;
    link(&object, &runtime, out).map_err(BuildError::Message)
}

/// The object file of the program of `file`: everything of a build but linking. Its image
/// records `toolchain`, the identity of the linker that links it ([`toolchain::identity`]).
pub fn compile_object(file: &Path, release: bool, toolchain: &str) -> Result<Vec<u8>, BuildError> {
    let Some(target) = Target::host() else {
        return Err(BuildError::Message(unsupported()));
    };
    compile_object_for(file, target, release, toolchain)
}

/// The object file of the program of `file` for `target`. Only an object for the host can be
/// linked with the runtime of this compiler; the others serve the tests of code generation.
pub fn compile_object_for(
    file: &Path,
    target: Target,
    release: bool,
    toolchain: &str,
) -> Result<Vec<u8>, BuildError> {
    let permissions = permissions_of(file).map_err(BuildError::Message)?;
    let file = file.to_path_buf();
    let toolchain = toolchain.to_string();
    crate::on_front_end_stack(move || {
        compile(&file, permissions.as_ref(), target, release, &toolchain)
    })
}

/// The permissions the manifest of the program's package declares, which the executable
/// expands when it starts (docs/runtime.md 8.4). `$HOME`, `$CWD`, and `$TEMP` then are those of
/// the run. A relative entry is refused: it is relative to the directory of the manifest
/// (T-6a), which an executable that is installed or moved does not have.
fn permissions_of(file: &Path) -> Result<Option<unio_runtime::sandbox::Declared>, String> {
    let Some((text, root)) = crate::sandbox_cli::manifest_of(file)? else {
        return Ok(None);
    };
    let manifest = root.join("unio.toml");
    let declared = unio_runtime::sandbox::declared_in_manifest(&text)
        .map_err(|e| format!("the manifest {} is invalid: {e}", manifest.display()))?;
    if let Some((key, entry)) = declared.relative_entries().first() {
        return Err(format!(
            "the manifest {} permits the relative path '{entry}' in {key}; an executable has no directory of its manifest when it runs, so write $CWD/{entry} for the working directory of the run, or an absolute path",
            manifest.display()
        ));
    }
    Ok(Some(declared))
}

/// The object file of the program: loads and checks it, lowers every body, writes the image.
fn compile(
    file: &Path,
    permissions: Option<&unio_runtime::sandbox::Declared>,
    target: Target,
    release: bool,
    toolchain: &str,
) -> Result<Vec<u8>, BuildError> {
    let program = crate::load_for_run(&[file.to_path_buf()], crate::effects::Profile::Application)
        .map_err(BuildError::Program)?;
    let interp = crate::interp::Interp::new(program);
    let units = units::collect(&interp);
    let image =
        image::encode(&interp, &units, permissions, toolchain).map_err(BuildError::Message)?;
    codegen::compile(
        &interp,
        &units,
        &image,
        &codegen::Options { release, target },
    )
    .map_err(BuildError::Message)
}

/// The runtime library (`libunio.a`, on Windows `unio.lib`): `UNIO_RUNTIME_LIB`, or the one
/// built next to this compiler.
pub fn runtime_library() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("UNIO_RUNTIME_LIB") {
        return Ok(PathBuf::from(p));
    }
    let name = Target::host().map_or("libunio.a", Target::runtime_file);
    let exe = std::env::current_exe().map_err(|e| format!("the compiler's location: {e}"))?;
    let candidate = exe
        .parent()
        .map(|d| d.join(name))
        .ok_or("the compiler's location")?;
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Test binaries live in target/<profile>/deps.
    if let Some(up) = exe.parent().and_then(Path::parent).map(|d| d.join(name)) {
        if up.is_file() {
            return Ok(up);
        }
    }
    Err(format!(
        "the runtime library {} is missing; build it with cargo build -p unio",
        candidate.display()
    ))
}

/// The name of the object file in the directory of a build, and of the executable there.
pub fn object_file_name() -> &'static str {
    if cfg!(windows) {
        "program.obj"
    } else {
        "program.o"
    }
}

pub fn executable_file_name() -> String {
    format!(
        "program{}",
        Target::host().map_or("", Target::executable_suffix)
    )
}

/// How the link step links: the linker, its arguments, the variables it needs besides `PATH`
/// and the temporary directory, and the trees it reads besides the system's library
/// directories (the SDK on macOS).
pub struct Linker {
    pub program: String,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub read: Vec<PathBuf>,
}

/// The linker of this machine and its arguments that link the object file with the runtime
/// into the executable `out`, with the system libraries the runtime needs (those that rustc
/// reports for the static library of this crate, `--print native-static-libs`):
///
/// - Linux: the C compiler (`cc`, or `CC`);
/// - macOS: `clang` of the selected developer tools (or `CC`) with their SDK, both found by
///   `xcrun` here, so that the link step runs the compiler and its linker directly and not
///   the `xcrun` shims of `/usr/bin`;
/// - Windows: `link.exe` of the newest Visual Studio C++ build tools, found as rustc finds it
///   (or from a developer command prompt), with the variables that locate its libraries.
pub fn link_command(object: &Path, runtime: &Path, out: &Path) -> Result<Linker, String> {
    let Some(target) = Target::host() else {
        return Err(unsupported());
    };
    let os = |items: &[&str]| -> Vec<OsString> { items.iter().map(OsString::from).collect() };
    match target {
        Target::LinuxX86_64 | Target::LinuxAarch64 => {
            let mut args: Vec<OsString> =
                vec!["-o".into(), out.into(), object.into(), runtime.into()];
            args.extend(os(&[
                "-Wl,--gc-sections",
                "-lgcc_s",
                "-lutil",
                "-lrt",
                "-lpthread",
                "-lm",
                "-ldl",
                "-lc",
            ]));
            Ok(Linker {
                program: linux_compiler(),
                args,
                env: Vec::new(),
                read: Vec::new(),
            })
        }
        Target::MacosAarch64 => {
            let program = macos_compiler()?;
            let sdk = PathBuf::from(xcrun(&["--show-sdk-path"])?);
            let mut args = os(&["-arch", "arm64", "-mmacosx-version-min=11.0", "-isysroot"]);
            args.push(sdk.clone().into());
            args.extend(["-o".into(), out.into(), object.into(), runtime.into()]);
            // No debug map: it would name the object file in the directory of this build.
            args.extend(os(&["-Wl,-dead_strip", "-Wl,-S", "-lSystem", "-lc", "-lm"]));
            Ok(Linker {
                program,
                args,
                env: Vec::new(),
                read: vec![sdk],
            })
        }
        Target::WindowsX86_64 => windows_linker(object, runtime, out),
    }
}

/// The C compiler that links on Linux: `CC`, or `cc`.
fn linux_compiler() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".to_string())
}

/// What `xcrun` prints for `what` (macOS), or why it failed.
fn xcrun(what: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("xcrun")
        .args(what)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("xcrun cannot be run ({e}); install the Xcode command line tools with xcode-select --install; unio does not install them"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || text.is_empty() {
        return Err(format!(
            "xcrun {} failed; install the Xcode command line tools with xcode-select --install:\n{}",
            what.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(text)
}

/// The compiler that links on macOS: `CC`, or `clang` of the selected developer tools.
fn macos_compiler() -> Result<String, String> {
    match std::env::var("CC") {
        Ok(cc) => Ok(cc),
        Err(_) => xcrun(&["--find", "clang"]),
    }
}

/// `link.exe` of Visual Studio and the variables that locate its libraries (`LIB`, `LIBPATH`,
/// `PATH`), as found on this machine.
pub struct MsvcLinker {
    pub path: PathBuf,
    pub env: Vec<(OsString, OsString)>,
}

/// How the `unio` tool finds `link.exe` on Windows: set once by the tool at start
/// (`find-msvc-tools`, compiler/README.md "The linker on Windows"). The finder lives in the tool
/// and not in this library, because the static library of this crate is the runtime linked
/// into every executable, and programs must not carry the search for a linker.
static MSVC_FINDER: std::sync::OnceLock<fn() -> Option<MsvcLinker>> = std::sync::OnceLock::new();

/// Sets the finder of `link.exe` (see [`MSVC_FINDER`]); a second call is ignored.
pub fn set_msvc_finder(finder: fn() -> Option<MsvcLinker>) {
    let _ = MSVC_FINDER.set(finder);
}

/// Why no executable is linked on Windows without `link.exe`.
#[cfg(windows)]
const MSVC_MISSING: &str =
    "the linker link.exe of Visual Studio was not found; install the Visual \
     Studio Build Tools with the workload \"Desktop development with C++\" (MSVC and a Windows \
     SDK), or run from a developer command prompt of the chosen installation; unio does not \
     install it";

/// `link.exe` as the finder of the tool finds it.
#[cfg(windows)]
fn find_msvc() -> Option<MsvcLinker> {
    MSVC_FINDER.get().and_then(|find| find())
}

#[cfg(windows)]
fn windows_linker(object: &Path, runtime: &Path, out: &Path) -> Result<Linker, String> {
    let tool = find_msvc().ok_or(MSVC_MISSING)?;
    let mut out_arg = OsString::from("/OUT:");
    out_arg.push(out);
    let mut args = vec![
        "/NOLOGO".into(),
        out_arg,
        "/SUBSYSTEM:CONSOLE".into(),
        "/OPT:REF".into(),
        "/DEBUG:NONE".into(),
        // The same executable from the same inputs (C-4): no time stamp.
        "/Brepro".into(),
        object.into(),
        runtime.into(),
    ];
    for lib in [
        "kernel32.lib",
        "ntdll.lib",
        "userenv.lib",
        "ws2_32.lib",
        "dbghelp.lib",
        "/DEFAULTLIB:msvcrt",
    ] {
        args.push(lib.into());
    }
    // LIB and LIBPATH locate the libraries of MSVC and the Windows SDK; PATH starts with the
    // directories of the tool, followed by that of this process.
    let env = tool.env;
    Ok(Linker {
        program: tool.path.to_string_lossy().into_owned(),
        args,
        env,
        read: Vec::new(),
    })
}

#[cfg(not(windows))]
fn windows_linker(_: &Path, _: &Path, _: &Path) -> Result<Linker, String> {
    Err(unsupported())
}

/// Links the object with the runtime by the linker of [`link_command`], in this process.
fn link(object: &[u8], runtime: &Path, out: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!(
        "unio-build-{}-{}",
        std::process::id(),
        out.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let obj = dir.join(object_file_name());
    std::fs::write(&obj, object).map_err(|e| format!("{}: {e}", obj.display()))?;
    let linker = link_command(&obj, runtime, out)?;
    let output = std::process::Command::new(&linker.program)
        .args(&linker.args)
        .envs(linker.env.iter().map(|(k, v)| (k, v)))
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let output = output.map_err(|e| format!("the linker {} cannot be run: {e}", linker.program))?;
    if !output.status.success() {
        return Err(format!(
            "linking failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine an object file is for, read from its header: (format, machine).
    fn header(object: &[u8]) -> (&'static str, u32) {
        let u16_at = |i: usize| u16::from_le_bytes([object[i], object[i + 1]]) as u32;
        let u32_at = |i: usize| u32::from_le_bytes(object[i..i + 4].try_into().unwrap());
        if object.starts_with(b"\x7fELF") {
            ("elf", u16_at(18))
        } else if u32_at(0) == 0xfeed_facf {
            ("macho", u32_at(4))
        } else {
            ("coff", u16_at(0))
        }
    }

    /// Every target gets the code of reference programs, verified by Cranelift, in its object
    /// format, the same in two builds (C-4). Linking and running happen on each target
    /// (tests/native.rs); this test checks the code generation of all four on any machine.
    #[test]
    fn every_target_gets_verified_code_in_its_object_format() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // Each object holds the standard library's units too, so a few programs cover the
        // operations of the reference programs: text, files, JSON, faults, and integer loops.
        let programs = [
            root.join("examples/wc.unio"),
            root.join("examples/json_query.unio"),
            root.join("bench/loop/loop.unio"),
        ];
        for program in &programs {
            // Loaded once per program, as `compile` does; code generated per target.
            let program_path = program.clone();
            let objects = crate::on_front_end_stack(move || {
                let loaded = crate::load_for_run(
                    std::slice::from_ref(&program_path),
                    crate::effects::Profile::Application,
                )
                .unwrap_or_else(|_| panic!("{} does not compile", program_path.display()));
                let interp = crate::interp::Interp::new(loaded);
                let units = units::collect(&interp);
                let image = image::encode(&interp, &units, None, "test toolchain").unwrap();
                let mut objects = Vec::new();
                for target in Target::ALL {
                    for release in [false, true, true] {
                        let object = codegen::compile(
                            &interp,
                            &units,
                            &image,
                            &codegen::Options { release, target },
                        );
                        objects.push((target, release, object));
                    }
                }
                objects
            });
            for chunk in objects.chunks(3) {
                let target = chunk[0].0;
                let expected = match target {
                    Target::LinuxX86_64 => ("elf", 62),
                    Target::LinuxAarch64 => ("elf", 183),
                    Target::MacosAarch64 => ("macho", 0x0100_000c),
                    Target::WindowsX86_64 => ("coff", 0x8664),
                };
                for (_, release, object) in chunk {
                    let object = object.as_ref().unwrap_or_else(|m| {
                        panic!("{} for {}: {m}", program.display(), target.name())
                    });
                    assert_eq!(
                        header(object),
                        expected,
                        "{} for {} (release {release})",
                        program.display(),
                        target.name()
                    );
                }
                assert!(
                    chunk[1].2 == chunk[2].2,
                    "two builds of {} for {} differ",
                    program.display(),
                    target.name()
                );
            }
        }
    }

    #[test]
    fn an_unsupported_machine_is_named() {
        let m = unsupported();
        for target in Target::ALL {
            assert!(m.contains(target.name()), "{m}");
        }
        assert!(m.contains(std::env::consts::OS), "{m}");
    }
}
