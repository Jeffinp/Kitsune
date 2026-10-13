//! `<video>` and `<audio>`: their boxes, the video surface command and the poster.

use super::*;

fn videos(p: &Page) -> Vec<(i32, i32, bool, bool)> {
    p.videos
        .iter()
        .map(|v| (v.w, v.h, v.controls, v.audio))
        .collect()
}

#[test]
fn a_video_without_a_size_is_300_by_150_and_takes_its_own_size_later() {
    let html = "<p>before</p><video src=/v.webm controls></video><p>after</p>";
    let p = lay(html, 800);
    assert_eq!(videos(&p), [(300, 150, true, false)]);
    let surface = p
        .cmds
        .iter()
        .find_map(|c| match c {
            Cmd::Video {
                w, h, uid, poster, ..
            } => Some((*w, *h, *uid, *poster)),
            _ => None,
        })
        .unwrap();
    assert_eq!((surface.0, surface.1, surface.3), (300, 150, None));
    assert_ne!(surface.2, 0, "the element's uid finds its player");
    // The text after it is below the box (it sits on a line of its own here).
    assert!(
        runs(&p)
            .iter()
            .any(|r| r.2 == "after" && r.1 > p.videos[0].y + 150)
    );
    // The player learnt the picture's size: the box follows.
    let mut doc = Doc::parse(html.as_bytes());
    assert!(doc.set_media_size(surface.2, 640, 360));
    assert!(!doc.set_media_size(surface.2, 640, 360), "no change");
    let p2 = doc.layout(&Layout {
        width: 800,
        zoom: 100,
        images: &NoImages,
        metrics: &FixedAdvance,
    });
    assert_eq!(videos(&p2), [(640, 360, true, false)]);
    // Narrower than the page: scaled down, ratio kept.
    let p3 = doc.layout(&Layout {
        width: 320,
        zoom: 100,
        images: &NoImages,
        metrics: &FixedAdvance,
    });
    assert_eq!(videos(&p3), [(304, 171, true, false)]);
}

#[test]
fn declared_sizes_and_the_aspect_ratio() {
    let p = lay("<video width=640></video>", 800);
    assert_eq!(videos(&p), [(640, 320, false, false)]);
    let p = lay("<video height=90></video>", 800);
    assert_eq!(videos(&p), [(180, 90, false, false)]);
    let p = lay("<video style='width:400px;height:100px'></video>", 800);
    assert_eq!(videos(&p), [(400, 100, false, false)]);
    let p = lay_zoom("<video></video>", 800, 200);
    assert_eq!(videos(&p), [(600, 300, false, false)]);
    // A block video sits on its own line.
    let p = lay(
        "<span>a</span><video style='display:block'></video><span>b</span>",
        800,
    );
    assert_eq!(p.videos.len(), 1);
    assert!(
        runs(&p)
            .iter()
            .any(|r| r.2 == "b" && r.1 >= p.videos[0].y + 150)
    );
}

#[test]
fn audio_is_a_bar_with_controls_and_nothing_without() {
    let p = lay(
        "<audio src=/a.webm controls></audio><audio src=/b.webm></audio>",
        800,
    );
    assert_eq!(videos(&p), [(300, 40, true, true)]);
}

#[test]
fn fallback_content_and_sources_are_not_shown_and_the_poster_is_a_picture() {
    let p = lay(
        "<video poster=/p.png controls><source src=/v.webm type=video/webm>Sem suporte</video>",
        800,
    );
    assert!(texts(&p).is_empty(), "{:?}", texts(&p));
    assert_eq!(p.images.len(), 1);
    assert_eq!(p.images[0].src, "/p.png");
    let poster = p.cmds.iter().find_map(|c| match c {
        Cmd::Video { poster, .. } => *poster,
        _ => None,
    });
    assert_eq!(poster, Some(0));
}

#[test]
fn videos_in_table_cells_move_with_their_cell() {
    let p = lay(
        "<table><tr><td><div style='height:400px'></div></td><td style='vertical-align:middle'><video></video></td></tr></table>",
        800,
    );
    let from_cmd = p.cmds.iter().find_map(|c| match c {
        Cmd::Video { y, .. } => Some(*y),
        _ => None,
    });
    assert_eq!(from_cmd, Some(p.videos[0].y));
    assert!(p.videos[0].y > 100, "moved to the middle of the cell");
    assert!(p.video_at(p.videos[0].x + 5, p.videos[0].y + 5).is_some());
    assert!(p.video_at(p.videos[0].x + 5, p.videos[0].y - 5).is_none());
}

#[test]
fn a_document_knows_it_has_media() {
    assert!(Doc::parse(b"<div><p><video></video></p></div>").has_media());
    assert!(Doc::parse(b"<audio src=x></audio>").has_media());
    assert!(!Doc::parse(b"<p>video</p><img src=v.png>").has_media());
}
