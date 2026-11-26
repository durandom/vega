use std::fs::File;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{CreateFileW, ReplaceFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

fn current_user_sid() -> io::Result<String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER points into this aligned buffer, which also holds the SID.
    const WORDS: usize = 1 + 68_usize.div_ceil(size_of::<TOKEN_USER>());
    let mut info = [MaybeUninit::<TOKEN_USER>::uninit(); WORDS];
    let mut size = 0;
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            info.as_mut_ptr().cast(),
            size_of::<[TOKEN_USER; WORDS]>() as u32,
            &mut size,
        )
    };
    let error = io::Error::last_os_error();
    unsafe { CloseHandle(token) };
    if result == 0 {
        return Err(error);
    }
    let mut sid = null_mut();
    if unsafe { ConvertSidToStringSidW(info[0].assume_init_ref().User.Sid, &mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // The API returns a NUL-terminated SID string allocated by LocalAlloc.
    let mut len = 0;
    unsafe {
        while *sid.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, len));
        LocalFree(sid.cast());
        Ok(text)
    }
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut path: Vec<_> = path.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    path.push(0);
    Ok(path)
}

/// Set the owner and protected DACL at creation, before writing any key bytes.
pub fn create_private(path: &Path) -> io::Result<File> {
    let path = wide_path(path)?;
    let sid = current_user_sid()?;
    let sddl: Vec<_> = format!("O:{sid}D:P(A;;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            &attrs,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    let error = io::Error::last_os_error();
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Replace a file atomically while preserving its DACL, or publish a new one.
pub fn replace_preserving_acl(path: &Path, replacement: &Path) -> io::Result<()> {
    let target = wide_path(path)?;
    let source = wide_path(replacement)?;
    if unsafe { ReplaceFileW(target.as_ptr(), source.as_ptr(), null(), 0, null(), null()) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::NotFound {
        return Err(error);
    }
    std::fs::hard_link(replacement, path)?;
    std::fs::remove_file(replacement)
}
