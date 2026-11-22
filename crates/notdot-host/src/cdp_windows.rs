//! Launch Chromium with only its two private debugging pipe handles inherited.
use std::{
    ffi::OsStr,
    io::{self, PipeReader, PipeWriter},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    process::Command,
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetCurrentProcess, InitializeProcThreadAttributeList, OpenProcess,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
    },
};

pub(super) struct Child {
    process: OwnedHandle,
    id: u32,
}
impl Child {
    pub(super) fn id(&self) -> u32 {
        self.id
    }
    pub(super) fn try_wait(&mut self) -> io::Result<Option<()>> {
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(Some(())),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub(super) fn kill(&mut self) -> io::Result<()> {
        check(unsafe { TerminateProcess(self.process.as_raw_handle(), 1) })
    }
    pub(super) fn wait(&mut self) -> io::Result<()> {
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 5000) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(io::ErrorKind::TimedOut.into()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

pub(super) fn process_alive(pid: u32) -> bool {
    // Unknown or access denied counts as alive so a profile is never swept on ambiguity.
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) }; // SYNCHRONIZE
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER as i32);
    }
    let mut child = Child {
        process: unsafe { OwnedHandle::from_raw_handle(handle) },
        id: pid,
    };
    !matches!(child.try_wait(), Ok(Some(())))
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn duplicate(handle: RawHandle) -> io::Result<OwnedHandle> {
    let mut copy = null_mut();
    // SAFETY: DuplicateHandle gives this process a new owned, inheritable handle.
    unsafe {
        let process = GetCurrentProcess();
        check(DuplicateHandle(
            process,
            handle,
            process,
            &mut copy,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        ))?;
        Ok(OwnedHandle::from_raw_handle(copy))
    }
}

pub(super) fn spawn(
    command: &Command,
    reader: &PipeReader,
    writer: &PipeWriter,
) -> io::Result<Child> {
    let reader = duplicate(reader.as_raw_handle())?;
    let writer = duplicate(writer.as_raw_handle())?;
    let handles: [HANDLE; 2] = [reader.as_raw_handle(), writer.as_raw_handle()];
    let pipes = format!(
        "--remote-debugging-io-pipes={},{}",
        handles[0] as usize, handles[1] as usize
    );
    let mut line = Vec::new();
    for arg in std::iter::once(command.get_program())
        .chain(command.get_args())
        .chain([OsStr::new(&pipes)])
    {
        let units: Vec<_> = arg.encode_wide().collect();
        // Current arguments are fixed strings or Windows paths, with no quotes
        // or trailing slash. Reject other shapes rather than misquote them.
        if units.contains(&0) || units.contains(&34) || units.last() == Some(&92) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        line.extend([34].into_iter().chain(units).chain([34, 32]));
    }
    line.push(0);
    let executable: Vec<_> = command.get_program().encode_wide().chain([0]).collect();
    struct Attributes(*mut core::ffi::c_void);
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }
    // SAFETY: the aligned backing buffer and handle array remain live for
    // CreateProcessW. Only the two duplicates enter the child. Both are
    // dropped by the caller once the process has been created.
    unsafe {
        let mut size = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        check(InitializeProcThreadAttributeList(list, 1, 0, &mut size))?;
        let attrs = Attributes(list);
        check(UpdateProcThreadAttribute(
            attrs.0,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of_val(&handles),
            null_mut(),
            null_mut(),
        ))?;
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attrs.0;
        let mut info: PROCESS_INFORMATION = std::mem::zeroed();
        check(CreateProcessW(
            executable.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &startup.StartupInfo,
            &mut info,
        ))?;
        drop(OwnedHandle::from_raw_handle(info.hThread));
        Ok(Child {
            process: OwnedHandle::from_raw_handle(info.hProcess),
            id: info.dwProcessId,
        })
    }
}
