//! HTML forms: what the parser finds ([`FormInfo`], [`FieldInfo`]), the state
//! the user edits ([`FormState`]), dead-key accent composition ([`Compose`])
//! and the GET query ([`FormState::target`]).
//!
//! Submitting builds `name=value&...` with `application/x-www-form-urlencoded`
//! escaping of the UTF-8 bytes: for `method=get` the browser navigates to
//! `action?query`, for `method=post` it sends the query as the request body
//! ([`FormState::submission`]). Supported controls: text-like inputs (`text`,
//! `search`, `url`, `email`, `tel`, `number` and no type), `password`,
//! `hidden`, `submit` and `<button>`, check boxes and radio buttons (a radio
//! group is the buttons of one form that share a `name`), `<select>` (a single
//! choice, changed by click or keys: there is no drop-down list) and `<textarea>`
//! (several lines; Enter inserts a line break). Files are not drawn.

use crate::Key;
use crate::tk;
use alloc::string::String;
use alloc::vec::Vec;

mod compose;
mod textedit;
pub use compose::*;
pub use textedit::*;

mod editing;
mod submit;
mod values;

/// Forms recorded per page.
pub const MAX_FORMS: usize = 16;
/// Controls recorded per form.
pub const MAX_FIELDS: usize = 32;
/// Longest value a text control holds, in bytes.
pub const MAX_VALUE: usize = 256;
/// Longest text a `<textarea>` holds, in bytes.
pub const MAX_TEXTAREA: usize = 4096;
/// Options kept per `<select>`.
pub const MAX_OPTIONS: usize = 256;
/// Longest query string built from a form for a GET, in bytes.
pub const MAX_QUERY: usize = 380;
/// Longest body a POST form sends, in bytes.
pub const MAX_POST: usize = 16 * 1024;
/// The kinds of control we draw and submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A one-line text box.
    Text,
    /// A text box that shows bullets.
    Password,
    /// Not drawn; always submitted.
    Hidden,
    /// A button; submitted only when it is the one pressed.
    Submit,
    /// A button that submits nothing (`type=button`, `type=reset`).
    PushButton,
    /// A check box: submitted (with its `value`) when checked.
    Checkbox,
    /// A radio button: one per group is checked.
    Radio,
    /// A `<select>`: one of its options is chosen.
    Select,
    /// A multi-line text box.
    TextArea,
}
impl FieldKind {
    /// Can the user type into it?
    pub fn is_text(self) -> bool {
        matches!(
            self,
            FieldKind::Text | FieldKind::Password | FieldKind::TextArea
        )
    }
}
/// One `<option>` of a `<select>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    /// What is sent (the `value` attribute, else the text).
    pub value: String,
    /// What is shown.
    pub label: String,
}
/// One control of a form, as written in the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    pub kind: FieldKind,
    /// The `value` attribute (the initial text of a text box).
    pub value: String,
    /// Width in characters of a text box.
    pub size: usize,
    /// The text on a button.
    pub label: String,
    /// The `checked` attribute (check boxes and radio buttons).
    pub checked: bool,
    /// The `placeholder` attribute of a text box.
    pub placeholder: String,
    /// The options of a `<select>`.
    pub options: Vec<SelectOption>,
    /// The option a `<select>` starts on.
    pub selected: usize,
    /// Visible lines of a `<textarea>`.
    pub rows: usize,
}
/// A `<form>` and its controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormInfo {
    /// `method=post`.
    pub post: bool,
    /// The `action` attribute (empty = the current page).
    pub action: String,
    pub fields: Vec<FieldInfo>,
    /// The controls outside any `<form>` of a page with scripts: drawn and usable, never submitted.
    pub formless: bool,
}
impl FormInfo {
    /// A form from its `method` and `action` attributes.
    pub fn from_attrs(method: Option<&str>, action: Option<String>) -> FormInfo {
        FormInfo {
            post: method.is_some_and(|m| m.trim().eq_ignore_ascii_case("post")),
            action: action.unwrap_or_default(),
            fields: Vec::new(),
            formless: false,
        }
    }

    /// The group of controls outside any form (see [`FormInfo::formless`]).
    pub fn formless() -> FormInfo {
        FormInfo {
            formless: true,
            ..FormInfo::from_attrs(None, None)
        }
    }
}
/// Why a form could not be submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormError {
    /// The query would not fit in a URL (or the body in a request).
    TooLong,
    /// No such form.
    NoForm,
}
impl FormError {
    /// Catalog key of the message shown to the user.
    pub fn message_key(self) -> &'static str {
        match self {
            FormError::TooLong => tk!("web.form.too_long"),
            FormError::NoForm => tk!("web.form.invalid"),
        }
    }

    /// The message in the language in effect.
    pub fn message(self) -> &'static str {
        crate::i18n::tr(self.message_key())
    }
}
/// Percent-encode `s` as an `application/x-www-form-urlencoded` component:
/// letters, digits and `*-._` stay, a space is `+`, every other UTF-8 byte is
/// `%XX` (upper-case hex).
pub fn urlencode(s: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 15) as usize] as char);
            }
        }
    }
}

/// What a key did to the focused control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormOutcome {
    /// Not handled (no focused control, or a key that means nothing here).
    Ignored,
    /// The control changed (repaint).
    Changed,
    /// Enter / a button: submit `form`, with `submitter` the button pressed (if any).
    Submit {
        form: usize,
        submitter: Option<usize>,
    },
    /// Esc: focus left the control.
    Blur,
}
/// The values the user typed and which control has the focus.
#[derive(Clone, Debug, Default)]
pub struct FormState {
    /// `values[form][field]`, starting as the `value` attributes.
    values: Vec<Vec<String>>,
    focus: Option<(usize, usize)>,
    /// Caret as a byte offset into the focused value (always a char boundary).
    caret: usize,
    compose: Compose,
}
/// What a form sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// Where to, relative to the page URL.
    pub href: String,
    /// The urlencoded body of a POST (`None`: a GET, the data is in `href`).
    pub body: Option<String>,
}
