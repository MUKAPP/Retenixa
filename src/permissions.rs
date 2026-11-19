use crate::error::Result;
use std::path::Path;
pub fn secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    restrict(path, true)
}
pub fn secure_file(path: &Path) -> Result<()> {
    restrict(path, false)
}
#[cfg(windows)]
fn restrict(path: &Path, directory: bool) -> Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree},
        Security::Authorization::{
            EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
            SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
        },
        Security::{
            DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token,
            TokenUser,
            storage.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            let e = std::io::Error::last_os_error();
            CloseHandle(token);
            return Err(e.into());
        }
        let user = &*(storage.as_ptr().cast::<TOKEN_USER>());
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: 0x1f01ff,
            grfAccessMode: SET_ACCESS,
            grfInheritance: if directory { 3 } else { 0 },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: user.User.Sid.cast(),
            },
        };
        let mut acl = ptr::null_mut();
        let result = SetEntriesInAclW(1, &entry, ptr::null(), &mut acl);
        if result != ERROR_SUCCESS {
            CloseHandle(token);
            return Err(std::io::Error::from_raw_os_error(result as i32).into());
        }
        let result = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        );
        LocalFree(acl.cast());
        CloseHandle(token);
        if result != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(result as i32).into());
        }
    }
    Ok(())
}
#[cfg(unix)]
fn restrict(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )?;
    Ok(())
}
