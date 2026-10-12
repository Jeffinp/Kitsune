use super::*;
use alloc::string::ToString;
use alloc::vec;

fn r(cwd: &str, p: &str) -> Option<String> {
    resolve_string(cwd, p)
}

#[test]
fn paths_resolve_against_the_working_directory() {
    assert_eq!(r("/home/a", "x.txt").unwrap(), "/home/a/x.txt");
    assert_eq!(r("/home/a", "/etc/x").unwrap(), "/etc/x");
    assert_eq!(r("/home/a", "./b/../c").unwrap(), "/home/a/c");
    assert_eq!(r("/home/a", "../..").unwrap(), "/");
    assert_eq!(r("/home/a", "../../../..").unwrap(), "/");
    assert_eq!(r("/", "").unwrap(), "/");
    assert_eq!(r("/a", "b//c/").unwrap(), "/a/b/c");
    assert_eq!(r("/a/b", "..").unwrap(), "/a");
    // A name that merely starts with dots is a name.
    assert_eq!(r("/", "..a/.b").unwrap(), "/..a/.b");
}

#[test]
fn bad_paths_are_refused() {
    assert!(r("/", "a\0b").is_none());
    assert!(r("/", &"a/".repeat(MAX_PATH)).is_none());
    let long = "x".repeat(MAX_PATH);
    assert!(r("/", &long).is_none());
    // Exactly the limit still fits.
    let ok = "y".repeat(MAX_PATH - 1);
    assert_eq!(r("/", &ok).unwrap().len(), MAX_PATH);
}

#[test]
fn arguments_round_trip() {
    let args = vec!["a b".to_string(), "".to_string(), "ç".to_string()];
    let blob = pack_args("/home/x", &args).unwrap();
    let (cwd, it) = unpack_args(&blob);
    assert_eq!(cwd, "/home/x");
    assert_eq!(it.collect::<Vec<_>>(), ["a b", "", "ç"]);
    let blob = pack_args("/", &[]).unwrap();
    let (cwd, it) = unpack_args(&blob);
    assert_eq!((cwd, it.count()), ("/", 0));
    assert!(pack_args("/", &["a\0".to_string()]).is_none());
    assert!(pack_args("/", &["x".repeat(MAX_ARGS)]).is_none());
}

#[test]
fn stat_records_round_trip() {
    let s = StatRec {
        kind: KIND_DIR,
        mode: 0o755,
        size: 1 << 40,
        mtime: 1_700_000_000,
        uid: 1000,
        gid: 7,
    };
    assert_eq!(StatRec::from_bytes(&s.to_bytes()), s);
    assert_eq!(core::mem::size_of::<StatRec>(), STAT_LEN);
}

#[test]
fn numbers_do_not_collide_with_the_other_calls() {
    let mine = [
        SYS_OPEN,
        SYS_CLOSE,
        SYS_READ,
        SYS_WRITE,
        SYS_SEEK,
        SYS_STAT,
        SYS_READDIR,
        SYS_MKDIR,
        SYS_REMOVE,
        SYS_RENAME,
        SYS_GETARGS,
    ];
    for (i, a) in mine.iter().enumerate() {
        assert!(*a >= 32, "{a} is in the range of the other calls");
        assert!(mine[i + 1..].iter().all(|b| b != a));
    }
}

#[test]
fn events_round_trip() {
    let e = Event {
        kind: EV_POINTER,
        a: -3,
        b: 799,
        c: 1,
    };
    assert_eq!(Event::from_bytes(&e.to_bytes()), e);
    assert_eq!(core::mem::size_of::<Event>(), EVENT_LEN);
    let kinds = [EV_KEY, EV_TEXT, EV_POINTER, EV_RESIZE, EV_CLOSE];
    for (i, a) in kinds.iter().enumerate() {
        assert!(kinds[i + 1..].iter().all(|b| b != a));
    }
}
