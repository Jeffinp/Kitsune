# Code structure

One folder per responsibility, a thin `mod.rs` per folder (declarations, re-exports and the folder's
documentation), tests next to the code. This page is the map; `kitsune_core/src/structure.rs` enforces
the dependency rules of the core.

## The tree

```
user-apps/                   ring 3 programs (detached workspaces): abi (shared contract, no deps) | sdk | demo | wasmhost
                             kterm | jsrun (QuickJS) | mediadec (WebM: libvpx VP9, libopus)
kitsune_core/src/            pure no_std logic, host-tested (forbid(unsafe_code))
  ui/         drawing and visual identity: gfx raster | ttf glyph fontcache textlayout | iconart appart
              brand pointer cursor | anim | style | widgets chrome layout | wallpaper
  security/   account password | perm | session                        (users, passwords, permissions)
  storage/    blockdev blockcache | fs (OJFS v2) fs3 (OJFS v3) | vfs secured
  network/    net dns icmp sntp lease netstats | x509 tlsverify
  browsing/   web (HTML/CSS/layout engine) | browser (model: tabs, history, errors) | redirect | shields (ad blocker)
              http1 (HTTP/1.1 framing) | cookies (the jar) | cors (same-origin rules)
              media (the <video>/<audio> player: ranges, track buffers, clock, state, messages, controls)
  format/     base64 | inflate deflate gzip zip | png bmp ppm gif jpeg image | elf | text | unixtime | search
              webm (Matroska demuxer) | md5
  platform/   appabi wasmsec | appmanifest appinstall | appfs | appnet      (the WASM app platform)
  system/     klog sysmon sysif | notify | process schedule paging heap | entropy rng | settings
              input keymap clipboard
  windowing/  window winman wm snap | compositor (damage engine) | taskbar launcher
  apps/       fileman viewer editor2 termui shell calc activity           (app logic, no pixels)
  hw/         device-independent driver logic (ata, pci, ps2, rtc, virtio, ac97, ...) and the sound mixer (audio)
  i18n/       catalogs, lookup, plurals, locale formatting
  structure.rs testutil.rs         (test-only)
  lib.rs      re-exports every module at the crate root (`kitsune_core::fs3`), see below

kernel/src/                  bare metal: drivers, scheduler, framebuffer, wasm host, desktop glue
  desktop/
    mod.rs        the `Desktop` struct, its constructor and the prelude shared by the folders
    frame.rs      per-frame stepping, the scheduler quantum, dirty-region bookkeeping
    compositor/   scene composition, layers, damage, present
    windows/      chrome, cursor, instance (Kind/App/Inst), lifecycle, geometry, drag, Alt+Tab
    shell/        model (Shell state, commands), step, panel/, taskbar/, overlays/ (Apps, Busca,
                  dialog), toasts, language-change hook
    input/        keys, clipboard, pointer (+ press, drag, hover), click, wheel
    kit/          ui/ (toolkit), charts, appui, appart, glass, widgets, live (hover clock)
    services/     vfs (desktop file API), sysstore (sysif implementations), shellhost/ (shelld)
    apps/<app>/   files viewer editor terminal browser tarefas registro ajustes calculadora wasm
                  gallery : state, input, paint (and a few concern files) in one folder
  wasm/ fb/ ...   other kernel subsystems (unchanged)
```

Names: folders are the Portuguese app names where the app is user-facing (`registro`, `ajustes`,
`tarefas`, `calculadora`), English elsewhere. Files are named by concern (`state`, `input`, `mouse`,
`keys`, `paint`, `step`, `layout`, `tests`), never by "ui" vs "logic". A file is split when it passes
~600 lines, along the existing `// ----` section seams. `mod.rs` holds only `mod` lines, re-exports and
the folder doc.

### Splitting a large file

A file over ~600 lines (tests excluded) becomes `foo.rs` (the doc, the imports, `mod` lines, the
re-exports and the few types the children share) plus a folder `foo/` with one child per subject; the
children start with `use super::*;`, so they see the parent's private items and imports. Items moved to a
child keep their visibility (`pub` stays `pub`, private becomes `pub(super)`, which is the same reach as
before) and the parent re-exports them with `use child::*;` (`pub use` for what the outside sees), so no
caller changes. A huge `impl` is split into several `impl` blocks, one child per subject (methods that were
private become `pub(super)`/`pub(in super::super)`). A module loaded with `#[path]` (`kernel/src/fb.rs`
includes `fb/shapes.rs`, and the bench crate includes `fb.rs`) names its children with `#[path]` too.

Files split this way, with their children:

| File | Children |
|---|---|
| `browsing/web/form` | `compose` (dead keys), `textedit`, `values`, `editing`, `submit` |
| `browsing/browser/addressbar` | `request`, `state`, `keys`, `navigation`, `internal`, `bookmarks`, `bar` |
| `browsing/web/{layout,style,css,dom}`, `format/svg` | see their folders |
| `format/jpeg` | `headers`, `scan`, `output` (+ `huffman`, `idct`, `progressive`) |
| `format/webm` | `ebml` (numbers and element headers), `ids`, `header` (Info, Tracks, Cues, OpusHead), `demux` (the incremental machine) |
| `ui/raster` | `color`, `corners`, `resample`, `shadow` |
| `ui/brand` | `head`, `halo`, `hints`, `render`, `svg` |
| `ui/anim` | `bezier`, `spring`, `transition`, `zoom`, `caret` |
| `ui/appart` | `files`, `tools` |
| `storage/fs3/mount` | `meta`, `inodes`, `txn` |
| `apps/shell/sys` | `types`, `mock` |
| `system/settings` | `timezones`, `choices`, `text`, `format` |
| `network/dns` | `message`, `cache`, `resolve` |
| `windowing/winman` | `lifecycle`, `focus`, `workspaces`, `geometry` (+ `helpers`) |
| `kernel/trace` | `stats`, `report`, `bench` |
| `kernel/sched` | `spawn`, `death`, `blocking`, `introspect`, `user` |
| `kernel/userproc` | `syscall` (entrada `int 0x80`), `host` (módulos WebAssembly), `files` (arquivos, janelas, filhos), `queue` (fila do sistema de arquivos e eventos de janela), `page` (script de página do navegador), `audio` (som, 60–65) |
| `i18n/audit` (testes) | `scan` (leitor das fontes), `accents` (acentos, idiomas, fontes) |
| `kernel/wasm/abi2` | `guest`, `hostio`, `pixels` |
| `kernel/netd` | `mailbox`, `pinger` |
| `kernel/fetch` | `client`, `thread`, `load` (+ `apps`, `jobs`, `cookies`, `script` with `script/serve`) |
| `kernel/netstack` | `http` (one exchange), `pool` (kept-alive connections), `tls` |
| `browsing/http1`, `browsing/cookies` | `reader`, `request`; `parse`, `jar`, `store` |
| `browsing/media` | `ranges`, `track`, `proto` (browser ↔ decoder), `clock`, `player` (+ `state`, `feed`), `wire` (page ↔ browser), `controls` |
| `kernel/desktop/apps/browser/media` | `paint`, `controls` (clicks), `stats`, `direct` (a `src` URL fetched into the decoder) |
| `kernel/fb/shapes` | `rrect`, `shadow`, `blit`, `glow` |

## Dependency rules (core)

`group -> allowed groups` (own group always allowed). Enforced by
`cargo test -p kitsune_core structure::`: an edge outside this table, a stale entry in the table and a
flat `crate::gfx` path used inside the crate all fail.

| Group | May use | Why |
|---|---|---|
| `format` | nothing | leaf: codecs and small utilities |
| `hw` | `network`, `storage` | drivers implement the block-device and MAC types |
| `i18n` | `hw` | locale formatting reads the RTC types |
| `security` | nothing | leaf: accounts, password hashing and permission rules take everything as arguments |
| `storage` | `security` | `secured` enforces the permission rules; storage never knows drawing, windows, network or apps |
| `network` | `format` | Unix time for certificate dates |
| `browsing` | `format`, `i18n`, `network`, `ui` | decoders, TLS, redirects, motion preference |
| `platform` | `browsing`, `format`, `i18n`, `storage` | HTTP response parsing, manifests, app volume |
| `ui` | `format`, `i18n`, `windowing` | wallpaper decoding; chrome is drawn around `Rect`/taskbar geometry |
| `windowing` | `format`, `i18n`, `ui` | style tokens and motion; substring search |
| `system` | `format`, `hw`, `i18n`, `ui`, `windowing` | settings use style/wallpaper, notify uses window geometry |
| `apps` | `format`, `i18n`, `platform`, `security`, `storage`, `system`, `ui`, `windowing` | app logic sits on top; nothing depends on `apps` |

The only mutual dependency between groups is `ui <-> windowing` (`anim`, `chrome`, `layout`, `widgets`,
`cursor` use `window::Rect` and the taskbar geometry; `window` uses `style`, `winman` uses `anim`).
A new edge that closes another cycle should be a design discussion, not a table edit.

The crate root re-exports every module under its old flat name (`kitsune_core::fs3` is
`kitsune_core::storage::fs3`) so the kernel, fuzz targets, benches and tools did not change. Inside the
crate, always use the grouped path (`crate::storage::fs3`).

## Dependency rules (kernel desktop)

`apps/*` may use `kit`, `services`, `shell` (menus, toasts), `windows` and `kitsune_core`; apps do not
use each other (shared things go to `kit` or `services`). `kit` and `services` do not use `apps`.
`windows` knows the app *kinds* (`instance.rs`) but not their internals. `input` dispatches to `apps`
through `Desktop` methods. Folder modules are `pub(super)`; what other folders need is re-exported in
the folder's `mod.rs`, and `desktop/mod.rs` keeps the short prelude (`ui`, `appui`, `vfs`, state types).

## Checklists

**New app** (a window kind):
1. Pure logic and its tests: `kitsune_core/src/apps/<app>.rs` + `<app>/tests.rs`.
2. `kernel/src/desktop/apps/<app>/` with `mod.rs` (doc, `mod` lines, re-export of the state type),
   `state.rs`, `input.rs`, `paint.rs`.
3. `windows/instance.rs`: a `Kind` variant with its metadata, an `App` variant, `App::new`.
4. `input/click.rs`, `input/keys.rs` (and wheel/hover) route to the new `Desktop` methods; `windows/chrome.rs`
   `draw_window` draws it; `shell/taskbar/state.rs` `DEFAULT_PINNED` if it starts pinned.
5. Strings: `assets/i18n/pt.txt` and `en.txt`; `cargo test -p kitsune_core i18n`.
6. `tools/verify-boot.sh` (desktop unchanged) and a scenario screenshot.

**New window kind that is not an app** (a tool window): same as an app, no launcher entry
(`Kind` metadata `multi`/`resizable`).

**New overlay or popover**: state in `shell/model.rs`, drawing in `shell/overlays/` (full screen) or
`shell/panel/popovers.rs` (hangs from the panel), entry point in `shell/overlays/layers.rs`, layer order in
`compositor/layers.rs`, keys in `shell/overlays/keys.rs`, pointer in `shell/overlays/pointer.rs`.

**New driver**: register-level code in `kernel/src/<driver>.rs`; parsing and state machines in
`kitsune_core/src/hw/<driver>.rs` with tests; IRQ handlers do not allocate or lock.

**New core module**: pick the group by responsibility (table above); `pub mod` in the group's `mod.rs`;
add the flat re-export to `lib.rs`; tests in `<module>/tests.rs`; if it needs a group that is not allowed,
rethink the placement before editing the table in `structure.rs` and this page.

**New WASM example**: `wasm-apps/examples/<name>/` (workspace-detached crate, SDK path `../../sdk`);
the images only bundle `clock`, `notes`, `paint` and `snake` (`BUNDLED_APPS` in `kernel/build.rs`).
