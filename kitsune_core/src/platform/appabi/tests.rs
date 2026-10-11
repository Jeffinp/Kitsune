use super::*;

#[test]
fn range_inside() {
    assert_eq!(check_range(100, 0, 100), Ok(0..100));
    assert_eq!(check_range(100, 10, 20), Ok(10..30));
    assert_eq!(check_range(100, 100, 0), Ok(100..100));
    assert_eq!(check_range(0, 0, 0), Ok(0..0));
}

#[test]
fn range_outside() {
    assert_eq!(check_range(100, 0, 101), Err(Fault));
    assert_eq!(check_range(100, 101, 0), Err(Fault));
    assert_eq!(check_range(100, 99, 2), Err(Fault));
    assert_eq!(check_range(0, 0, 1), Err(Fault));
}

#[test]
fn negative_values_are_huge_not_small() {
    assert_eq!(check_range(1 << 20, -1, 1), Err(Fault));
    assert_eq!(check_range(1 << 20, 0, -1), Err(Fault));
    assert!(check_range(usize::MAX, i32::MIN, 0).is_ok());
    assert_eq!(check_range(100, i32::MIN, 1), Err(Fault));
}

#[test]
fn no_overflow_at_the_extremes() {
    assert_eq!(
        check_range(usize::MAX, -1, -1),
        Ok(u32::MAX as usize..(u32::MAX as u64 * 2) as usize)
    );
    assert_eq!(check_range(10, i32::MAX, i32::MAX), Err(Fault));
}

#[test]
fn capped_variants() {
    assert_eq!(check_capped(100, 0, 10, 16), Ok(Ok(0..10)));
    assert_eq!(check_capped(100, 0, 17, 16), Ok(Err(ERR_INVAL)));
    assert_eq!(check_capped(100, 200, 17, 16), Err(Fault));
    assert_eq!(check_capped(100, 0, -5, 16), Err(Fault));
    assert_eq!(check_capped(100, 95, 10, 16), Err(Fault));
}

#[test]
fn error_codes_are_distinct_and_negative() {
    let all = [
        ERR_PERM,
        ERR_NOENT,
        ERR_BADF,
        ERR_INVAL,
        ERR_EXIST,
        ERR_NOSPC,
        ERR_MFILE,
        ERR_NOTDIR,
        ERR_ISDIR,
        ERR_NOTEMPTY,
        ERR_NET,
        ERR_NOSYS,
    ];
    for (i, a) in all.iter().enumerate() {
        assert!(*a < 0);
        for b in &all[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn open_flags_are_disjoint_bits() {
    assert_eq!(O_READ | O_WRITE | O_CREATE | O_TRUNC | O_APPEND, O_ALL);
    assert_eq!(O_ALL, 31);
}

mod remote {
    use super::super::remote::*;

    #[test]
    fn the_table_has_unique_names_and_matches_the_abi() {
        for (i, f) in HOST_FNS.iter().enumerate() {
            assert_eq!(host_fn_id(f.name), Some(i as u32), "{}", f.name);
            assert!(
                f.params.len() <= 6,
                "{}: at most six arguments cross the boundary",
                f.name
            );
            assert!(f.results.len() <= 1);
        }
        assert_eq!(host_fn_id("nope"), None);
        assert_eq!(
            HOST_FNS[host_fn_id("fs_seek").unwrap() as usize]
                .params
                .len(),
            3
        );
    }

    #[test]
    fn records_round_trip_through_their_bytes() {
        let j = Job {
            kind: JOB_CALL,
            export: 3,
            args: [-1, 2, 3, 4],
            fuel: 1 << 40,
            mem_bytes: 7,
            win: 0x0000_0500_0000_0320,
            lang: 1,
        };
        assert_eq!(Job::from_bytes(&j.to_bytes()), j);
        let h = HostCall {
            id: 9,
            nargs: 2,
            args: [1, 2, u64::MAX, 4, 5, 6],
            mem_base: 0x1000_0000,
            mem_len: 65536,
        };
        assert_eq!(HostCall::from_bytes(&h.to_bytes()), h);
        let d = Done {
            status: DONE_EXIT,
            code: -5,
            fuel_spent: 99,
            mem_pages: 3,
            msg_ptr: 1,
            msg_len: 2,
        };
        assert_eq!(Done::from_bytes(&d.to_bytes()), d);
        assert_eq!(core::mem::size_of::<Job>(), 56);
        assert_eq!(core::mem::size_of::<HostCall>(), 72);
        assert_eq!(core::mem::size_of::<Done>(), 40);
    }

    #[test]
    fn only_calls_without_result_or_memory_are_batchable() {
        let ids: alloc::vec::Vec<_> = (0..HOST_FNS.len() as u32)
            .filter(|i| batchable(*i))
            .collect();
        assert!(!ids.is_empty());
        for id in ids {
            let f = &HOST_FNS[id as usize];
            assert!(f.results.is_empty(), "{} returns a value", f.name);
            assert!(f.params.len() <= 5);
        }
        assert!(batchable(host_fn_id("fill_rect").unwrap()));
        assert!(!batchable(host_fn_id("draw_text").unwrap()));
        assert!(!batchable(host_fn_id("exit").unwrap()));
        assert!(!batchable(9999));
        const { assert!(MAX_BATCH * 72 <= 4608) };
    }
}
