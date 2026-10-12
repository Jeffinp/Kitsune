//! `ShellFs` over the file calls of the kernel: every operation acts with the rights of the user
//! who started the terminal.

use alloc::string::String;
use alloc::vec::Vec;
use kitsune_core::shell::fs::{DirEntry, FileMeta, FsErr, Kind, ShellFs, Stat};
use kitsune_user::abi::{self, KIND_DIR, errno};
use kitsune_user::fs::{self, File};

pub struct UserFs {
    cwd: String,
}

impl UserFs {
    pub fn new(cwd: &str) -> UserFs {
        UserFs {
            cwd: String::from(cwd),
        }
    }
}

pub fn map_err(e: i64) -> FsErr {
    match e {
        errno::ENOENT => FsErr::NotFound,
        errno::EEXIST => FsErr::AlreadyExists,
        errno::ENOTDIR => FsErr::NotADirectory,
        errno::EISDIR => FsErr::IsADirectory,
        errno::ENOTEMPTY => FsErr::NotEmpty,
        errno::EINVAL => FsErr::InvalidPath,
        errno::ENAMETOOLONG => FsErr::NameTooLong,
        errno::ENOSPC | errno::EMFILE => FsErr::NoSpace,
        errno::EFBIG => FsErr::TooBig,
        errno::EACCES => FsErr::PermissionDenied,
        _ => FsErr::Io,
    }
}

impl ShellFs for UserFs {
    fn cwd(&self) -> String {
        self.cwd.clone()
    }

    fn set_cwd(&mut self, path: &str) -> Result<(), FsErr> {
        let p = self.resolve(path);
        match self.stat(&p)?.kind {
            Kind::Dir => {
                self.cwd = p;
                Ok(())
            }
            Kind::File => Err(FsErr::NotADirectory),
        }
    }

    fn stat(&self, path: &str) -> Result<Stat, FsErr> {
        let r = fs::stat(&self.resolve(path)).map_err(map_err)?;
        Ok(Stat {
            kind: if r.kind == KIND_DIR {
                Kind::Dir
            } else {
                Kind::File
            },
            size: r.size,
        })
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, FsErr> {
        let p = self.resolve(path);
        let mut f = File::open(&p).map_err(map_err)?;
        let mut out = Vec::new();
        let mut buf = alloc::vec![0u8; abi::MAX_IO];
        loop {
            let n = f.read(&mut buf).map_err(map_err)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, FsErr> {
        let p = self.resolve(path);
        let mut f = File::open(&p).map_err(map_err)?;
        f.seek(offset as i64, abi::SEEK_SET).map_err(map_err)?;
        let mut out = Vec::new();
        let mut buf = alloc::vec![0u8; abi::MAX_IO.min(len.max(1))];
        while out.len() < len {
            let want = buf.len().min(len - out.len());
            let n = f.read(&mut buf[..want]).map_err(map_err)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), FsErr> {
        let p = self.resolve(path);
        let mut f = File::create(&p).map_err(map_err)?;
        f.write_all(data).map_err(map_err)
    }

    fn append(&mut self, path: &str, data: &[u8]) -> Result<(), FsErr> {
        let p = self.resolve(path);
        let mut f = File::append(&p).map_err(map_err)?;
        f.write_all(data).map_err(map_err)
    }

    fn list(&self, path: &str) -> Result<Vec<DirEntry>, FsErr> {
        let dir = self.resolve(path);
        let mut out = Vec::new();
        let mut name = [0u8; 256];
        let mut i = 0;
        while let Some((kind, n)) = fs::read_dir(&dir, i, &mut name).map_err(map_err)? {
            let n = String::from_utf8_lossy(n).into_owned();
            let kind = if kind == KIND_DIR {
                Kind::Dir
            } else {
                Kind::File
            };
            let size = if kind == Kind::File {
                let full = if dir == "/" {
                    alloc::format!("/{n}")
                } else {
                    alloc::format!("{dir}/{n}")
                };
                fs::stat(&full).map_or(0, |s| s.size)
            } else {
                0
            };
            out.push(DirEntry {
                name: n,
                kind,
                size,
            });
            i += 1;
        }
        Ok(out)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsErr> {
        fs::mkdir(&self.resolve(path)).map_err(map_err)
    }

    fn remove(&mut self, path: &str) -> Result<(), FsErr> {
        let p = self.resolve(path);
        if self.stat(&p)?.kind == Kind::Dir && !self.list(&p)?.is_empty() {
            return Err(FsErr::NotEmpty);
        }
        fs::remove(&p).map_err(map_err)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsErr> {
        let (a, b) = (self.resolve(from), self.resolve(to));
        if a == b {
            return Ok(());
        }
        if let Ok(st) = self.stat(&b) {
            if st.kind == Kind::Dir {
                return Err(FsErr::AlreadyExists);
            }
            fs::remove(&b).map_err(map_err)?;
        }
        fs::rename(&a, &b).map_err(map_err)
    }

    fn meta(&self, path: &str) -> Option<FileMeta> {
        let r = fs::stat(&self.resolve(path)).ok()?;
        Some(FileMeta {
            uid: r.uid,
            gid: r.gid,
            mode: r.mode as u16,
        })
    }
}
