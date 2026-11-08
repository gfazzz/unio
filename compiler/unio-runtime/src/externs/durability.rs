//! Which file systems keep the guarantees of requirement S-9 (S-9a): ext4, XFS, and Btrfs on
//! Linux, APFS on macOS, NTFS on Windows. Elsewhere `files.open_atomic(durable = true)` fails
//! with kind Unsupported.

use std::path::{Path, PathBuf};

/// The file systems that keep the guarantees.
pub const SUPPORTED: &[&str] = &["ext4", "xfs", "btrfs", "apfs", "ntfs"];

/// The directory whose file system decides: the path if it is a directory, otherwise its
/// parent (the target of a write may not exist yet).
fn directory_of(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Whether the file system that holds the path keeps the guarantees of S-9.
pub fn supports_durability(path: &Path) -> bool {
    let directory = match std::fs::canonicalize(crate::sandbox::absolute(&directory_of(path))) {
        Ok(d) => d,
        Err(_) => return false,
    };
    match file_system_name(&directory) {
        Some(name) => SUPPORTED.contains(&name.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// The name of the file system that holds a directory.
#[cfg(target_os = "linux")]
pub fn file_system_name(directory: &Path) -> Option<String> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    file_system_from_mountinfo(&mountinfo, directory.to_str()?)
}

/// The file system of the mount point that is the longest prefix of the directory, from the
/// text of /proc/self/mountinfo.
pub fn file_system_from_mountinfo(mountinfo: &str, directory: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let (before, after) = match line.split_once(" - ") {
            Some(parts) => parts,
            None => continue,
        };
        let mount_point = match before.split(' ').nth(4) {
            Some(m) => unescape_mount(m),
            None => continue,
        };
        let file_system = match after.split(' ').next() {
            Some(f) => f.to_string(),
            None => continue,
        };
        let inside = directory == mount_point
            || mount_point == "/"
            || directory
                .strip_prefix(mount_point.as_str())
                .is_some_and(|rest| rest.starts_with('/'));
        if inside
            && best
                .as_ref()
                .is_none_or(|(length, _)| mount_point.len() >= *length)
        {
            best = Some((mount_point.len(), file_system));
        }
    }
    best.map(|(_, file_system)| file_system)
}

/// mountinfo writes space, tab, newline, and backslash in mount points as octal escapes.
fn unescape_mount(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_vendor = "apple")]
pub fn file_system_name(directory: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};
    use std::os::unix::ffi::OsStrExt;

    // struct statfs with 64-bit inodes, as the C library of Apple platforms defines it.
    #[repr(C)]
    struct StatFs {
        f_bsize: u32,
        f_iosize: i32,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_fsid: [i32; 2],
        f_owner: u32,
        f_type: u32,
        f_flags: u32,
        f_fssubtype: u32,
        f_fstypename: [c_char; 16],
        f_mntonname: [c_char; 1024],
        f_mntfromname: [c_char; 1024],
        f_flags_ext: u32,
        f_reserved: [u32; 7],
    }

    extern "C" {
        #[cfg_attr(target_arch = "x86_64", link_name = "statfs$INODE64")]
        fn statfs(path: *const c_char, buf: *mut StatFs) -> c_int;
    }

    let path = CString::new(directory.as_os_str().as_bytes()).ok()?;
    // SAFETY: StatFs matches the C layout and is fully written by a successful call.
    let mut buffer: StatFs = unsafe { std::mem::zeroed() };
    let result = unsafe { statfs(path.as_ptr(), &mut buffer) };
    if result != 0 {
        return None;
    }
    let name: Vec<u8> = buffer
        .f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(name).ok()
}

#[cfg(windows)]
pub fn file_system_name(directory: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumePathNameW(file_name: *const u16, volume_path: *mut u16, length: u32) -> i32;
        fn GetVolumeInformationW(
            root: *const u16,
            volume_name: *mut u16,
            volume_name_size: u32,
            serial_number: *mut u32,
            maximum_component_length: *mut u32,
            flags: *mut u32,
            file_system_name: *mut u16,
            file_system_name_size: u32,
        ) -> i32;
    }

    let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut root = vec![0u16; 1024];
    let mut name = vec![0u16; 64];
    // SAFETY: the buffers are as long as the lengths passed, and the strings end with 0.
    unsafe {
        if GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
            return None;
        }
        if GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        ) == 0
        {
            return None;
        }
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16(&name[..end]).ok()
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
pub fn file_system_name(_: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
30 22 0:25 / /home rw,relatime shared:2 - btrfs /dev/sda2 rw
31 22 0:26 / /mnt/usb\\040stick rw,relatime shared:3 - vfat /dev/sdb1 rw
32 22 0:27 / /tmp rw shared:4 - tmpfs tmpfs rw";

    #[test]
    fn the_longest_mount_point_decides() {
        let fs = |d| file_system_from_mountinfo(MOUNTINFO, d);
        assert_eq!(fs("/etc").as_deref(), Some("ext4"));
        assert_eq!(fs("/home/anna/notes").as_deref(), Some("btrfs"));
        assert_eq!(fs("/home").as_deref(), Some("btrfs"));
        assert_eq!(fs("/homework").as_deref(), Some("ext4"));
        assert_eq!(fs("/mnt/usb stick/a").as_deref(), Some("vfat"));
        assert_eq!(fs("/tmp/x").as_deref(), Some("tmpfs"));
    }

    #[test]
    fn the_answer_for_this_machine_matches_its_file_system() {
        let here = std::env::temp_dir();
        let expected = file_system_name(&std::fs::canonicalize(&here).unwrap())
            .map(|n| SUPPORTED.contains(&n.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        assert_eq!(supports_durability(&here.join("not-yet-created")), expected);
    }
}
