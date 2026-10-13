//! Print the `init` message the browser would send to the script process for a page (the scripts
//! follow it in messages of their own, not printed):
//! `script_init PAGE.html URL [NAME=FILE.js ...]` (the external scripts, by the name the page uses).
//! For trying the page mode of `jsrun` on the host; see docs/design/tls-browser.md (§12).
use kitsune_core::web::Doc;
use kitsune_core::web::script::{InitInfo, ScriptSet, init_message};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let html = std::fs::read(&args[1]).expect("page");
    let url = &args[2];
    let doc = Doc::parse(&html);
    let mut set = ScriptSet::default();
    set.begin(&doc, url.as_bytes());
    while let Some(key) = set.next_pending() {
        let file = args[3..].iter().find_map(|a| {
            a.split_once('=')
                .filter(|(n, _)| key.ends_with(n))
                .map(|x| x.1)
        });
        let body = file.and_then(|f| std::fs::read(f).ok());
        set.finish(&key, body.as_deref());
    }
    let scripts = set.take();
    let m = init_message(
        &doc,
        &InitInfo {
            url,
            lang: "pt-BR",
            cookie: "",
            referrer: "",
            width: 800,
            height: 600,
        },
        scripts.len(),
    );
    std::io::stdout().write_all(&m).unwrap();
}
