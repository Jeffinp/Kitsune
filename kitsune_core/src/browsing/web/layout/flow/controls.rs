//! Form controls in the flow: registering them in their form and queueing their boxes.

use super::*;

/// Register the form control `el` (an `<input>`, `<button>`, `<select>` or `<textarea>`) in the current
/// form and queue its box. Returns `true` when the element was consumed (a
/// control, or something to skip), `false` to let it render as ordinary content.
pub(crate) fn collect_control(p: &mut Painter, el: &Element, c: &Computed) -> bool {
    let attr = |n: &str| el.attrs.get(n).map(String::as_str);
    let ty = attr("type").unwrap_or("").trim().to_ascii_lowercase();
    let name = decode_attr(attr("name").unwrap_or(""));
    let mut value = decode_attr(attr("value").unwrap_or(""));
    let mut options = Vec::new();
    let mut selected = 0;
    let mut rows = 0;
    let placeholder = fold_display(&decode_attr(attr("placeholder").unwrap_or("")));
    let checked = el.attrs.contains_key("checked");
    let (kind, label, chars) = if el.tag == "select" {
        selected = collect_options(&el.children, &mut options).unwrap_or(0);
        let widest = options
            .iter()
            .map(|o| o.label.chars().count())
            .max()
            .unwrap_or(0);
        (FieldKind::Select, String::new(), widest.clamp(6, 40))
    } else if el.tag == "textarea" {
        let n = |name: &str, default: usize, max: usize| {
            attr(name)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(default)
                .min(max)
        };
        rows = n("rows", 2, 12);
        // The text between the tags is the initial value; one line break right after the
        // opening tag is not part of it.
        let t = text_content(&el.children);
        let t = t
            .strip_prefix("\r\n")
            .or_else(|| t.strip_prefix('\n'))
            .unwrap_or(&t);
        value = t.replace('\r', "");
        (FieldKind::TextArea, String::new(), n("cols", 20, 80).max(8))
    } else if el.tag == "button" {
        match ty.as_str() {
            "" | "submit" => {
                let t = fold_display(text_content(&el.children).trim());
                (
                    FieldKind::Submit,
                    if t.is_empty() { "Enviar".into() } else { t },
                    0,
                )
            }
            "button" | "reset" => {
                let t = fold_display(text_content(&el.children).trim());
                (FieldKind::PushButton, t, 0)
            }
            _ => return false,
        }
    } else {
        match ty.as_str() {
            "hidden" => (FieldKind::Hidden, String::new(), 0),
            "" | "text" | "search" | "url" | "email" | "tel" | "number" => {
                let n = attr("size")
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .filter(|&n| n > 0)
                    .unwrap_or(20)
                    .min(80);
                (FieldKind::Text, String::new(), n)
            }
            "password" => (FieldKind::Password, String::new(), 20),
            "submit" => {
                let l = if value.is_empty() {
                    "Enviar".into()
                } else {
                    fold_display(&value)
                };
                (FieldKind::Submit, l, 0)
            }
            "button" | "reset" => (FieldKind::PushButton, fold_display(&value), 0),
            "checkbox" => (FieldKind::Checkbox, String::new(), 0),
            "radio" => (FieldKind::Radio, String::new(), 0),
            // file, image, range, color, date, ...: not supported, not drawn.
            _ => return true,
        }
    };
    let form = match (p.cur_form, p.formless) {
        (Some(f), _) | (None, Some(f)) => f,
        // Outside any form a control is drawn only on a page with scripts (they can make it do
        // something); it joins a form of its own that is never submitted.
        (None, None) if p.track && p.forms.len() < crate::browsing::web::form::MAX_FORMS => {
            p.forms.push(FormInfo::formless());
            p.formless = Some(p.forms.len() - 1);
            p.forms.len() - 1
        }
        _ => return true, // a control outside any <form> has nowhere to go
    };
    let kind = if p.cur_form.is_none() && kind == FieldKind::Submit {
        FieldKind::PushButton
    } else {
        kind
    };
    let f = &mut p.forms[form];
    if f.fields.len() >= crate::browsing::web::form::MAX_FIELDS {
        return true;
    }
    let field = f.fields.len();
    let value = if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) && value.is_empty() {
        String::from("on")
    } else {
        value
    };
    f.fields.push(FieldInfo {
        name,
        kind,
        value,
        size: chars,
        label: label.clone(),
        checked,
        placeholder,
        options,
        selected,
        rows,
    });
    if kind != FieldKind::Hidden {
        let st = p.style(c);
        p.items.push(Item::Obj {
            obj: Obj::Field {
                form,
                field,
                kind,
                chars,
                rows,
                label,
            },
            st,
            sp: p.space.take(),
        });
    }
    true
}

/// The `<option>`s under a `<select>` (also inside `<optgroup>`; an option that was never
/// closed holds the ones after it, which the parser already ends). Returns the index of the
/// first one marked `selected`.
pub(crate) fn collect_options(
    nodes: &[crate::browsing::web::dom::Node],
    out: &mut Vec<crate::browsing::web::form::SelectOption>,
) -> Option<usize> {
    use crate::browsing::web::dom::Node;
    let mut chosen = None;
    for n in nodes {
        let Node::Element(e) = n else { continue };
        if out.len() >= crate::browsing::web::form::MAX_OPTIONS {
            break;
        }
        if e.tag == "option" {
            let label = fold_display(text_content(&e.children).trim());
            let value = match e.attrs.get("value") {
                Some(v) => decode_attr(v),
                None => label.clone(),
            };
            if e.attrs.contains_key("selected") && chosen.is_none() {
                chosen = Some(out.len());
            }
            out.push(crate::browsing::web::form::SelectOption { value, label });
        } else if let Some(i) = collect_options(&e.children, out) {
            chosen = chosen.or(Some(i));
        }
    }
    chosen
}
