use crate::browsing::media::controls::*;

#[test]
fn parts_are_where_clicks_find_them() {
    let l = layout(640, 360, 100);
    assert_eq!(l.bar.y, 360 - BAR_H);
    assert_eq!(l.hit(10, 10), None, "above the bar: the video itself");
    let c = |a: Area| (a.x + a.w / 2, a.y + a.h / 2);
    let (x, y) = c(l.play);
    assert_eq!(l.hit(x, y), Some(Part::Play));
    let (x, y) = c(l.seek);
    assert_eq!(l.hit(x, y), Some(Part::Seek));
    let (x, y) = c(l.mute);
    assert_eq!(l.hit(x, y), Some(Part::Mute));
    let (x, y) = c(l.volume);
    assert_eq!(l.hit(x, y), Some(Part::Volume));
    // In order from left to right, inside the box.
    assert!(l.play.x + l.play.w <= l.time.x);
    assert!(l.time.x + l.time.w <= l.seek.x);
    assert!(l.seek.x + l.seek.w <= l.mute.x);
    assert!(l.mute.x + l.mute.w <= l.volume.x);
    assert!(l.volume.x + l.volume.w <= 640);
}

#[test]
fn narrow_boxes_drop_the_volume_and_the_time() {
    let l = layout(200, 100, 100);
    assert_eq!(l.volume.w, 0);
    assert_eq!(l.time.w, 0);
    assert!(l.seek.w > 0);
    let z = layout(640, 360, 200);
    assert_eq!(z.bar.h, 2 * BAR_H);
}

#[test]
fn seeking_and_volume_from_the_click() {
    let l = layout(640, 360, 100);
    let d = 3_000_000_000;
    assert_eq!(l.seek_time(l.seek.x, d), 0);
    assert_eq!(l.seek_time(l.seek.x + l.seek.w, d), d);
    let mid = l.seek_time(l.seek.x + l.seek.w / 2, d);
    assert!((mid - d / 2).abs() < d / i64::from(l.seek.w) + 1);
    assert_eq!(l.seek_time(-50, d), 0);
    assert_eq!(l.seek_time(l.seek.x + 5, 0), 0);
    assert_eq!(l.volume_at(l.volume.x), 0);
    assert_eq!(l.volume_at(l.volume.x + l.volume.w), 100);
    assert_eq!(l.progress_px(d / 2, d), l.seek.w / 2);
    assert_eq!(l.progress_px(2 * d, d), l.seek.w);
}

#[test]
fn the_picture_is_fitted_into_its_box() {
    // Same shape: the whole box.
    assert_eq!(
        contain(640, 360, 1280, 720),
        Area {
            x: 0,
            y: 0,
            w: 640,
            h: 360
        }
    );
    // 16:9 in the default 300x150 (2:1): bars at the sides.
    assert_eq!(
        contain(300, 150, 640, 360),
        Area {
            x: 17,
            y: 0,
            w: 266,
            h: 150
        }
    );
    // 4:3 in a 16:9 box... and a tall box: bars above and below.
    assert_eq!(
        contain(200, 400, 640, 360),
        Area {
            x: 0,
            y: 144,
            w: 200,
            h: 112
        }
    );
    // Nothing known: the box.
    assert_eq!(
        contain(300, 150, 0, 0),
        Area {
            x: 0,
            y: 0,
            w: 300,
            h: 150
        }
    );
}

#[test]
fn time_text() {
    assert_eq!(fmt_time(0), "0:00");
    assert_eq!(fmt_time(3_400_000_000), "0:03");
    assert_eq!(fmt_time(61_000_000_000), "1:01");
    assert_eq!(fmt_time(3_725_000_000_000), "1:02:05");
    assert_eq!(
        super::super::controls::time_text(1_000_000_000, None),
        "0:01 / --:--"
    );
    assert_eq!(
        super::super::controls::time_text(0, Some(3_000_000_000)),
        "0:00 / 0:03"
    );
}
