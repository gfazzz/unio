//! External functions of the `std` layer for standard output, arguments, the environment, and
//! paths.

use super::{path, text, ExternFn};
use crate::fault::Outcome;
use crate::runtime::Runtime;
use crate::value::Value;

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("write_output", write_output),
    ("write_error", write_error),
    ("std.args.list", args_list),
    ("std.env.get", env_get),
    ("std.env.home_dir", env_home_dir),
    ("Path", path_new),
    ("Path.to_text", path_to_text),
    ("separator", separator),
];

/// Writes the text to standard output; a closed output loses it (write_output cannot fail).
fn write_output(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    runtime.write_output(t.as_str());
    Ok(Value::Unit)
}

/// Writes the text to standard error.
fn write_error(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    runtime.write_error(t.as_str());
    Ok(Value::Unit)
}

/// The arguments of the program without its name: those after `--` of `unio run`, or those of
/// a built executable.
fn args_list(runtime: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::list(
        runtime
            .args
            .iter()
            .map(|a| Value::text(a.clone()))
            .collect(),
    ))
}

/// The value of an environment variable; none if it is not set, not permitted, or not valid
/// UTF-8.
fn env_get(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let name = text(runtime, arguments, 0)?;
    let value = std::env::var_os(name.as_str()).and_then(|v| v.into_string().ok());
    Ok(Value::option(value.map(Value::text)))
}

/// The home directory as the operating system reports it, not from $HOME.
fn env_home_dir(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::option(home_dir().map(Value::path)))
}

#[cfg(unix)]
fn home_dir() -> Option<String> {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    // The fields up to pw_dir of struct passwd; the layouts of Linux and Apple differ.
    #[cfg(not(target_vendor = "apple"))]
    #[repr(C)]
    struct Passwd {
        pw_name: *mut c_char,
        pw_passwd: *mut c_char,
        pw_uid: u32,
        pw_gid: u32,
        pw_gecos: *mut c_char,
        pw_dir: *mut c_char,
    }

    #[cfg(target_vendor = "apple")]
    #[repr(C)]
    struct Passwd {
        pw_name: *mut c_char,
        pw_passwd: *mut c_char,
        pw_uid: u32,
        pw_gid: u32,
        pw_change: i64,
        pw_class: *mut c_char,
        pw_gecos: *mut c_char,
        pw_dir: *mut c_char,
    }

    extern "C" {
        fn getuid() -> u32;
        fn getpwuid(uid: u32) -> *mut Passwd;
    }

    // SAFETY: getpwuid returns a pointer to static storage or null; it is read immediately,
    // before any other call could overwrite it, on the thread of the interpreter.
    unsafe {
        let entry = getpwuid(getuid());
        if entry.is_null() || (*entry).pw_dir.is_null() {
            return None;
        }
        let directory = CStr::from_ptr((*entry).pw_dir).to_str().ok()?;
        if directory.is_empty() {
            None
        } else {
            Some(directory.to_string())
        }
    }
}

/// On Windows the profile directory of the user, as `std::env::home_dir` reports it.
#[cfg(windows)]
#[allow(deprecated)]
fn home_dir() -> Option<String> {
    std::env::home_dir().and_then(|p| p.into_os_string().into_string().ok())
}

#[cfg(not(any(unix, windows)))]
fn home_dir() -> Option<String> {
    None
}

fn path_new(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Path(text(runtime, arguments, 0)?))
}

fn path_to_text(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Text(path(runtime, arguments, 0)?))
}

/// "/" on Linux and macOS, "\" on Windows.
fn separator(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::text(std::path::MAIN_SEPARATOR.to_string()))
}

#[cfg(test)]
mod tests {
    use crate::externs::call;
    use crate::runtime::Runtime;
    use crate::value::Value;

    #[test]
    fn output_arguments_and_paths() {
        let (mut rt, out, err) = Runtime::captured(vec!["a".into(), "ё".into()]);
        call(&mut rt, "write_output", &mut [Value::text("hello\n")]).unwrap();
        call(&mut rt, "write_error", &mut [Value::text("oops\n")]).unwrap();
        assert_eq!(&out.borrow()[..], b"hello\n");
        assert_eq!(&err.borrow()[..], b"oops\n");
        assert_eq!(
            call(&mut rt, "std.args.list", &mut []).unwrap(),
            Value::list(vec![Value::text("a"), Value::text("ё")])
        );
        let path = call(&mut rt, "Path", &mut [Value::text("dir/file.txt")]).unwrap();
        assert_eq!(
            call(&mut rt, "Path.to_text", &mut [path]).unwrap(),
            Value::text("dir/file.txt")
        );
        let separator = call(&mut rt, "separator", &mut []).unwrap();
        assert!(separator == Value::text("/") || separator == Value::text("\\"));
    }

    #[test]
    fn environment() {
        let mut rt = Runtime::new(vec![]);
        assert_eq!(
            call(
                &mut rt,
                "std.env.get",
                &mut [Value::text("UNIO_SURELY_UNSET_VARIABLE")]
            )
            .unwrap(),
            Value::none()
        );
        let path = std::env::var("PATH").unwrap_or_default();
        if !path.is_empty() {
            assert_eq!(
                call(&mut rt, "std.env.get", &mut [Value::text("PATH")]).unwrap(),
                Value::some(Value::text(path))
            );
        }
        match call(&mut rt, "std.env.home_dir", &mut [])
            .unwrap()
            .field("value")
        {
            Some(Value::Path(p)) => assert!(!p.is_empty()),
            other => assert!(other.is_none()),
        }
    }
}
