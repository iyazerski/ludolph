use std::ffi::{CStr, c_char, c_int};
use std::os::fd::RawFd;

use anyhow::{Result, bail};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

// `notify(3)`, part of libSystem, which the `libc` crate leaves out.
unsafe extern "C" {
    fn notify_register_file_descriptor(
        name: *const c_char,
        notify_fd: *mut c_int,
        flags: c_int,
        out_token: *mut c_int,
    ) -> u32;
    fn notify_cancel(token: c_int) -> u32;
}

/// Posts of one `notify(3)` name, such as a power or network change.
#[derive(Debug)]
pub struct Changes {
    fd: Option<AsyncFd<RawFd>>,
    token: c_int,
}

impl Changes {
    pub fn new(name: &CStr) -> Result<Self> {
        let (mut fd, mut token) = (-1, -1);
        // SAFETY: the name is NUL-terminated, and both out pointers are
        // valid. Flags 0 asks for a new descriptor.
        let status = unsafe {
            notify_register_file_descriptor(name.as_ptr(), &raw mut fd, 0, &raw mut token)
        };
        if status != 0 {
            bail!("registering for {name:?} failed with status {status}");
        }
        let mut changes = Changes { fd: None, token };
        // SAFETY: `fd` is the open descriptor notify(3) just made.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            let error = std::io::Error::last_os_error();
            bail!("can't make the {name:?} notification non-blocking: {error}");
        }
        // SAFETY: `fd` stays open until `drop` cancels the registration,
        // which is after the `AsyncFd` is gone.
        let fd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) };
        changes.fd = Some(fd.map_err(|e| e.into_parts().1)?);
        Ok(changes)
    }

    pub async fn next(&mut self) -> Result<()> {
        let fd = self.fd.as_ref().expect("set until dropped");
        loop {
            let mut ready = fd.readable().await?;
            // Drain all four-byte tokens so a burst becomes one change.
            let mut drained = false;
            loop {
                let mut buffer = [0u8; 64];
                // SAFETY: `buffer` is valid for its length.
                let read =
                    unsafe { libc::read(*fd.get_ref(), buffer.as_mut_ptr().cast(), buffer.len()) };
                match read {
                    n if n > 0 => drained = true,
                    0 => bail!("the notification closed"),
                    _ => {
                        let error = std::io::Error::last_os_error();
                        if error.kind() != std::io::ErrorKind::WouldBlock {
                            return Err(error.into());
                        }
                        ready.clear_ready();
                        break;
                    }
                }
            }
            if drained {
                return Ok(());
            }
        }
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        // Cancel closes the fd; deregister AsyncFd first because RawFd does not own it.
        drop(self.fd.take());
        // SAFETY: `token` came from `notify_register_file_descriptor`.
        unsafe { notify_cancel(self.token) };
    }
}
