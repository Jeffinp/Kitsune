//! Tests of the file manager logic.

use super::*;
use crate::storage::blockdev::RamDisk;
use crate::storage::fs3::{FormatOptions, Fs3};
use alloc::vec;

const NOW: u64 = 1_700_000_000;

fn fresh() -> Fs3<RamDisk> {
    Fs3::format(
        RamDisk::new(8 * 2048),
        &FormatOptions::new(*b"0123456789abcdef", NOW),
    )
    .unwrap()
}

fn row(name: &str, dir: bool, size: u64, mtime: u64) -> Row {
    Row {
        name: name.as_bytes().to_vec(),
        kind: if dir { EntryKind::Dir } else { EntryKind::File },
        size,
        mtime,
        id: Vec::new(),
        installed: false,
    }
}

fn names(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| String::from_utf8_lossy(&r.name).into_owned())
        .collect()
}

fn sel(n: usize) -> Selection {
    let mut s = Selection::new();
    s.reset(n);
    s
}

fn populated() -> Fs3<RamDisk> {
    let mut fs = fresh();
    fs.mkdir("/Docs", NOW).unwrap();
    fs.mkdir("/Docs/inner", NOW).unwrap();
    fs.write_file("/Docs/a.txt", b"aaa", NOW + 1).unwrap();
    fs.write_file("/Docs/b.png", b"x", NOW + 2).unwrap();
    fs.write_file("/z.txt", b"zz", NOW + 3).unwrap();
    fs
}

fn items() -> Vec<apps::AppItem> {
    let it = |id: &str, name: &str, installed: bool, size: u64| apps::AppItem {
        id: id.into(),
        name: name.into(),
        installed,
        size,
    };
    alloc::vec![
        it("snake", "Snake", true, 3000),
        it("notes", "Notas", true, 5200),
        it("paint", "Pintura", false, 9000),
        it("clock", "Relógio", true, 2100),
    ]
}

fn searchable() -> FileView {
    let mut fs = fresh();
    for n in [
        "Ação.txt",
        "relatorio.pdf",
        "RELATÓRIO final.txt",
        "foto.png",
    ] {
        fs.write_file(alloc::format!("/{n}").as_str(), b"x", NOW)
            .unwrap();
    }
    fs.mkdir("/Relatórios", NOW).unwrap();
    let mut v = FileView::new();
    v.refresh(&mut fs).unwrap();
    v
}

mod apps_place;
mod archive;
mod breadcrumbs_history;
mod classification;
mod clipboard;
mod context_menu;
mod formatting;
mod natural_order;
mod places;
mod search_filter;
mod selection;
mod sorting;
mod text_input;
mod text_input_selection;
mod view_over_real;
