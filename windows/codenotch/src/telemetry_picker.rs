#[cfg(windows)]
pub fn choose(initial: Option<String>, owner: isize) -> Result<Option<String>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};

    struct Apartment;
    impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok().map_err(|e| e.to_string())?;
    let _apartment = Apartment;
    let dialog: IFileOpenDialog = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }.map_err(|e| e.to_string())?;
    unsafe {
        let options = dialog.GetOptions().map_err(|e| e.to_string())?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).map_err(|e| e.to_string())?;
        let title: Vec<u16> = "Codenotch — pasta da telemetria".encode_utf16().chain(Some(0)).collect();
        dialog.SetTitle(PCWSTR(title.as_ptr())).map_err(|e| e.to_string())?;
        if let Some(initial) = initial {
            let mut folder = std::path::PathBuf::from(initial);
            while !folder.is_dir() && folder.pop() {}
            if folder.is_dir() {
                let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
                let item: windows::core::Result<IShellItem> = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None);
                if let Ok(item) = item { let _ = dialog.SetDefaultFolder(&item); }
            }
        }
        if let Err(error) = dialog.Show(HWND(owner as _)) {
            if error.code().0 as u32 == 0x800704C7 { return Ok(None); }
            return Err(error.to_string());
        }
        let result = dialog.GetResult().map_err(|e| e.to_string())?;
        let path = result.GetDisplayName(SIGDN_FILESYSPATH).map_err(|e| e.to_string())?;
        let value = path.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(path.0 as _));
        value.map(Some)
    }
}

#[cfg(not(windows))]
pub fn choose(_initial: Option<String>, _owner: isize) -> Result<Option<String>, String> {
    Err("Use the folder field on this platform".into())
}
