//! archives and encodings: `gzip`, `gunzip`, `zip`, `unzip` and `iconv` over [`crate::format::gzip`] and
//! [`crate::format::zip`]. Whole files are held in memory, so every read and write is capped.

use super::*;
use crate::format::gzip::{self as gz, EncodingError};
use crate::format::text::{self, Encoding};
use crate::format::unixtime::DateTime;
use crate::format::zip::{self, ZipError};

/// Largest file read, compressed or expanded.
const MAX_FILE: usize = 16 << 20;
/// Largest archive built.
const MAX_ARCHIVE: usize = 32 << 20;

/// Create `dir` and the folders above it that are missing.
fn make_dirs(cx: &mut CmdCtx<'_>, dir: &str) -> Result<(), (String, FsErr)> {
    let abs = cx.fs.resolve(dir);
    let mut cur = String::new();
    for comp in abs.split('/').filter(|c| !c.is_empty()) {
        cur.push('/');
        cur.push_str(comp);
        match cx.fs.stat(&cur) {
            Ok(s) if s.kind == Kind::Dir => {}
            Ok(_) => return Err((cur, FsErr::NotADirectory)),
            Err(_) => cx.fs.mkdir(&cur).map_err(|e| (cur.clone(), e))?,
        }
    }
    Ok(())
}

fn gz_error(e: EncodingError, path: &str) -> String {
    match e {
        EncodingError::BadHeader => t!("sh.arch.not_gzip", path = path),
        EncodingError::Truncated => t!("sh.arch.truncated", path = path),
        EncodingError::TooLarge => t!("sh.arch.too_large", path = path, mb = MAX_FILE >> 20),
        EncodingError::Corrupt | EncodingError::BadChecksum => {
            t!("sh.arch.corrupt", path = path)
        }
    }
}

fn gz_run(cx: &mut CmdCtx<'_>, decompress: bool) -> i32 {
    let Some(o) = parse_opts(cx, "dkcf", "", None) else {
        return 2;
    };
    let decompress = decompress || o.has('d');
    let to_stdout = o.has('c');
    let transform = |data: &[u8], name: &str| -> Result<Vec<u8>, String> {
        if decompress {
            gz::gunzip(data, MAX_FILE).map_err(|e| gz_error(e, name))
        } else if data.len() > MAX_FILE {
            Err(t!("sh.arch.too_large", path = name, mb = MAX_FILE >> 20))
        } else {
            Ok(gz::gzip(data))
        }
    };
    if o.operands.is_empty() {
        let stdin = cx.stdin.to_vec();
        return match transform(&stdin, "stdin") {
            Ok(out) => {
                cx.out(&out);
                0
            }
            Err(msg) => {
                cx.error(&msg);
                1
            }
        };
    }
    let mut status = 0;
    for f in &o.operands {
        match cx.fs.stat(f) {
            Ok(s) if s.kind == Kind::Dir => {
                cx.error(&t!("sh.arch.is_dir", path = f.as_str()));
                status = 1;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                fs_err(cx, f, e);
                status = 1;
                continue;
            }
        }
        let dest = if decompress {
            match f.strip_suffix(".gz") {
                Some(d) if !d.is_empty() => d.to_string(),
                _ => {
                    cx.error(&t!("sh.arch.bad_suffix", path = f.as_str()));
                    status = 1;
                    continue;
                }
            }
        } else if f.ends_with(".gz") {
            cx.error(&t!("sh.arch.has_suffix", path = f.as_str()));
            status = 1;
            continue;
        } else {
            format!("{f}.gz")
        };
        let data = match cx.fs.read(f) {
            Ok(d) => d,
            Err(e) => {
                fs_err(cx, f, e);
                status = 1;
                continue;
            }
        };
        let out = match transform(&data, f) {
            Ok(o) => o,
            Err(msg) => {
                cx.error(&msg);
                status = 1;
                continue;
            }
        };
        if to_stdout {
            cx.out(&out);
            continue;
        }
        if !o.has('f') && cx.fs.stat(&dest).is_ok() {
            cx.error(&t!("sh.arch.exists", path = dest.as_str()));
            status = 1;
            continue;
        }
        if let Err(e) = cx.fs.write(&dest, &out) {
            let msg = t!(
                "sh.err.cannot_create",
                path = dest.as_str(),
                why = e.message()
            );
            cx.error(&msg);
            status = 1;
            continue;
        }
        if !o.has('k') {
            let _ = cx.fs.remove(f);
        }
    }
    status
}

pub(super) fn gzip(cx: &mut CmdCtx<'_>) -> i32 {
    gz_run(cx, false)
}

pub(super) fn gunzip(cx: &mut CmdCtx<'_>) -> i32 {
    gz_run(cx, true)
}

/// An archive being built.
struct ZipJob {
    w: zip::ZipWriter,
    /// The archive's own path, which is never added to itself.
    skip: String,
    recursive: bool,
    dos: (u16, u16),
}

/// Add `abs` (named `name` in the archive) and, with `job.recursive`, what is under it.
fn add_tree(
    cx: &mut CmdCtx<'_>,
    job: &mut ZipJob,
    abs: &str,
    name: &str,
    depth: usize,
) -> Result<(), String> {
    let ZipJob { w, skip, dos, .. } = job;
    let (skip, dos) = (skip.as_str(), *dos);
    if abs == skip {
        return Ok(());
    }
    let st = cx
        .fs
        .stat(abs)
        .map_err(|e| t!("sh.err.cannot_stat", path = abs, why = e.message()))?;
    let full = |e: ZipError| match e {
        ZipError::TooLarge => t!("sh.arch.too_large", path = name, mb = MAX_ARCHIVE >> 20),
        _ => t!("sh.zip.too_many", path = name),
    };
    if st.kind == Kind::File {
        let data = cx
            .fs
            .read(abs)
            .map_err(|e| t!("sh.err.cannot_read", path = abs, why = e.message()))?;
        if data.len() > MAX_FILE || w.bytes() + data.len() > MAX_ARCHIVE {
            return Err(t!("sh.arch.too_large", path = name, mb = MAX_FILE >> 20));
        }
        w.add_file(name, &data, dos).map_err(full)?;
        cx.println(&t!("sh.zip.added", name = name));
        return Ok(());
    }
    w.add_dir(name, dos).map_err(full)?;
    cx.println(&t!("sh.zip.added", name = format!("{name}/").as_str()));
    if !job.recursive {
        return Ok(());
    }
    if depth > 32 {
        return Err(t!("sh.cp.too_deep", path = abs));
    }
    let mut entries = cx
        .fs
        .list(abs)
        .map_err(|e| t!("sh.err.cannot_read", path = abs, why = e.message()))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for e in entries {
        add_tree(
            cx,
            job,
            &join(abs, &e.name),
            &format!("{name}/{}", e.name),
            depth + 1,
        )?;
    }
    Ok(())
}

pub(super) fn zip(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "r", "", None) else {
        return 2;
    };
    if o.operands.len() < 2 {
        cx.error(t!("sh.zip.usage"));
        return 2;
    }
    let mut archive = o.operands[0].clone();
    if !archive.to_ascii_lowercase().ends_with(".zip") {
        archive.push_str(".zip");
    }
    let skip = cx.fs.resolve(&archive);
    let now = cx.sys.now();
    let dos = zip::dos_time(&DateTime {
        year: i32::from(now.year),
        month: now.month,
        day: now.day,
        hour: now.hour,
        minute: now.minute,
        second: now.second,
    });
    let mut job = ZipJob {
        w: zip::ZipWriter::new(),
        skip,
        recursive: o.has('r'),
        dos,
    };
    let mut status = 0;
    for p in &o.operands[1..] {
        if matches!(p.as_str(), "." | "./") {
            // The working folder itself: its contents, without a name for it.
            let here = cx.fs.resolve(".");
            let mut kids = cx.fs.list(&here).unwrap_or_default();
            kids.sort_by(|a, b| a.name.cmp(&b.name));
            for k in kids {
                let abs = join(&here, &k.name);
                if let Err(msg) = add_tree(cx, &mut job, &abs, &k.name, 0) {
                    cx.error(&msg);
                    status = 1;
                }
            }
            continue;
        }
        let Some(name) = zip::safe_path(p) else {
            cx.error(&t!("sh.unzip.unsafe", name = p.as_str()));
            status = 1;
            continue;
        };
        let name = name.trim_end_matches('/').to_string();
        let abs = cx.fs.resolve(p);
        if let Err(msg) = add_tree(cx, &mut job, &abs, &name, 0) {
            cx.error(&msg);
            status = 1;
        }
    }
    if job.w.is_empty() {
        return status.max(1);
    }
    let bytes = job.w.finish();
    if let Err(e) = cx.fs.write(&archive, &bytes) {
        let msg = t!(
            "sh.err.cannot_create",
            path = archive.as_str(),
            why = e.message()
        );
        cx.error(&msg);
        return 1;
    }
    status
}

fn zip_error(e: ZipError, name: &str) -> String {
    match e {
        ZipError::NotZip => t!("sh.unzip.not_zip", path = name),
        ZipError::Truncated => t!("sh.arch.truncated", path = name),
        ZipError::Unsupported => t!("sh.unzip.unsupported", name = name),
        ZipError::BadChecksum => t!("sh.unzip.bad_crc", name = name),
        ZipError::TooLarge => t!("sh.arch.too_large", path = name, mb = MAX_FILE >> 20),
        ZipError::Corrupt | ZipError::TooMany => t!("sh.arch.corrupt", path = name),
    }
}

fn stamp(dos: (u16, u16)) -> String {
    match zip::from_dos(dos.0, dos.1) {
        Some(DateTime {
            year,
            month,
            day,
            hour,
            minute,
            ..
        }) => format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"),
        None => String::from("----------------"),
    }
}

pub(super) fn unzip(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "lo", "d", None) else {
        return 2;
    };
    let Some(archive) = o.operands.first().cloned() else {
        cx.error(t!("sh.unzip.usage"));
        return 2;
    };
    let members = &o.operands[1..];
    let data = match cx.fs.read(&archive) {
        Ok(d) => d,
        Err(e) => {
            fs_err(cx, &archive, e);
            return 1;
        }
    };
    if data.len() > MAX_ARCHIVE {
        cx.error(&t!(
            "sh.arch.too_large",
            path = archive.as_str(),
            mb = MAX_ARCHIVE >> 20
        ));
        return 1;
    }
    let entries = match zip::read_dir(&data) {
        Ok(e) => e,
        Err(e) => {
            cx.error(&zip_error(e, &archive));
            return 1;
        }
    };
    let chosen: Vec<&zip::Entry> = entries
        .iter()
        .filter(|e| {
            members.is_empty()
                || members.iter().any(|m| {
                    let m = m.trim_end_matches('/');
                    e.name.trim_end_matches('/') == m || e.name.starts_with(&format!("{m}/"))
                })
        })
        .collect();
    let mut status = 0;
    for m in members {
        if !chosen.iter().any(|e| {
            e.name.trim_end_matches('/') == m.trim_end_matches('/')
                || e.name.starts_with(&format!("{}/", m.trim_end_matches('/')))
        }) {
            cx.error(&t!("sh.unzip.no_member", name = m.as_str()));
            status = 1;
        }
    }
    if o.has('l') {
        cx.println(t!("sh.unzip.list_header"));
        let mut total = 0u64;
        for e in &chosen {
            total += u64::from(e.size);
            cx.println(&format!("{:>10}  {}  {}", e.size, stamp(e.dos), e.name));
        }
        cx.println(&t!(
            "sh.unzip.total",
            n = chosen.len(),
            size = total.to_string().as_str()
        ));
        return status;
    }
    let root = o
        .value('d')
        .map_or_else(|| cx.fs.cwd(), |d| cx.fs.resolve(d));
    if let Err((p, e)) = make_dirs(cx, &root) {
        fs_err(cx, &p, e);
        return 1;
    }
    for e in chosen {
        let Some(rel) = zip::safe_path(&e.name) else {
            cx.error(&t!("sh.unzip.unsafe", name = e.name.as_str()));
            status = 1;
            continue;
        };
        let target = join(&root, rel.trim_end_matches('/'));
        if e.is_dir {
            if let Err((p, err)) = make_dirs(cx, &target) {
                fs_err(cx, &p, err);
                status = 1;
            }
            continue;
        }
        if !o.has('o') && cx.fs.stat(&target).is_ok() {
            cx.error(&t!("sh.unzip.exists", path = target.as_str()));
            status = 1;
            continue;
        }
        let bytes = match zip::extract(&data, e, MAX_FILE) {
            Ok(b) => b,
            Err(err) => {
                cx.error(&zip_error(err, &e.name));
                status = 1;
                continue;
            }
        };
        let parent = crate::apps::shell::fs::parent(&target).to_string();
        if let Err((p, err)) = make_dirs(cx, &parent) {
            fs_err(cx, &p, err);
            status = 1;
            continue;
        }
        match cx.fs.write(&target, &bytes) {
            Ok(()) => cx.println(&t!("sh.unzip.inflating", path = target.as_str())),
            Err(err) => {
                let msg = t!(
                    "sh.err.cannot_create",
                    path = target.as_str(),
                    why = err.message()
                );
                cx.error(&msg);
                status = 1;
            }
        }
        if cx.full() {
            break;
        }
    }
    status
}

pub(super) fn iconv(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "", "ft", None) else {
        return 2;
    };
    let (Some(from), Some(to)) = (o.value('f'), o.value('t')) else {
        cx.error(t!("sh.iconv.usage"));
        return 2;
    };
    let (from, to) = (from.to_string(), to.to_string());
    let (Some(src), Some(dst)) = (Encoding::parse(&from), Encoding::parse(&to)) else {
        let bad = if Encoding::parse(&from).is_none() {
            from
        } else {
            to
        };
        cx.error(&t!("sh.iconv.unknown", name = bad.as_str()));
        return 1;
    };
    let (inputs, mut ok) = inputs(cx, &o.operands);
    for (name, data) in inputs {
        let utf8 = text::decode_as(&data, src);
        match text::encode(&utf8, dst) {
            Some(mut out) => {
                // `iconv -t UTF-16LE` writes no byte-order mark.
                if matches!(dst, Encoding::Utf16Le | Encoding::Utf16Be) {
                    out.drain(..2);
                }
                cx.out(&out);
            }
            None => {
                cx.error(&t!(
                    "sh.iconv.cannot",
                    name = name.as_str(),
                    to = dst.label()
                ));
                ok = false;
            }
        }
    }
    i32::from(!ok)
}
