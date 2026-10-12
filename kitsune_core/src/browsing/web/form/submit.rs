//! What a form sends: the urlencoded query, the GET target and the POST submission.

use super::*;

impl FormState {
    /// The `name=value&...` query of `form` when `submitter` was pressed.
    pub fn query(
        &self,
        forms: &[FormInfo],
        form: usize,
        submitter: Option<usize>,
    ) -> Result<String, FormError> {
        self.encode(forms, form, submitter, MAX_QUERY)
    }

    pub(super) fn encode(
        &self,
        forms: &[FormInfo],
        form: usize,
        submitter: Option<usize>,
        limit: usize,
    ) -> Result<String, FormError> {
        let f = forms.get(form).ok_or(FormError::NoForm)?;
        let mut q = String::new();
        for (i, fi) in f.fields.iter().enumerate() {
            let include = match fi.kind {
                FieldKind::Hidden | FieldKind::Text | FieldKind::Password | FieldKind::TextArea => {
                    !fi.name.is_empty()
                }
                FieldKind::Select => !fi.name.is_empty() && !fi.options.is_empty(),
                FieldKind::Submit => submitter == Some(i) && !fi.name.is_empty(),
                FieldKind::Checkbox | FieldKind::Radio => {
                    !fi.name.is_empty() && self.is_checked(form, i)
                }
                FieldKind::PushButton => false,
            };
            if !include {
                continue;
            }
            if !q.is_empty() {
                q.push('&');
            }
            urlencode(&fi.name, &mut q);
            q.push('=');
            match fi.kind {
                FieldKind::Hidden | FieldKind::Submit | FieldKind::Checkbox | FieldKind::Radio => {
                    urlencode(fi.value.as_str(), &mut q)
                }
                FieldKind::Select => {
                    let o = fi.options.get(self.select_index(form, i));
                    urlencode(o.map_or("", |o| o.value.as_str()), &mut q)
                }
                // A browser sends a line break as CR LF.
                FieldKind::TextArea => {
                    urlencode(&self.value(form, i).replace('\n', "\r\n"), &mut q)
                }
                _ => urlencode(self.value(form, i), &mut q),
            }
            if q.len() > limit {
                return Err(FormError::TooLong);
            }
        }
        Ok(q)
    }

    /// What submitting `form` sends: the address (relative to the page URL: the form's action
    /// without its own query or fragment; empty stays on the current page) and, for
    /// `method=post`, the body. A GET puts the query in the address after `?`.
    pub fn submission(
        &self,
        forms: &[FormInfo],
        form: usize,
        submitter: Option<usize>,
    ) -> Result<Submission, FormError> {
        let f = forms.get(form).ok_or(FormError::NoForm)?;
        let action = f.action.trim();
        let end = action.find(['?', '#']).unwrap_or(action.len());
        let mut href = String::from(&action[..end]);
        if f.post {
            // A POST keeps the query string of its action (it is part of the address).
            let q_end = action.find('#').unwrap_or(action.len());
            href = String::from(&action[..q_end]);
            let body = self.encode(forms, form, submitter, MAX_POST)?;
            return Ok(Submission {
                href,
                body: Some(body),
            });
        }
        let q = self.encode(forms, form, submitter, MAX_QUERY)?;
        href.push('?');
        href.push_str(&q);
        Ok(Submission { href, body: None })
    }

    /// The `href` to navigate to on submit for a GET form (see [`FormState::submission`]).
    pub fn target(
        &self,
        forms: &[FormInfo],
        form: usize,
        submitter: Option<usize>,
    ) -> Result<String, FormError> {
        self.submission(forms, form, submitter).map(|s| s.href)
    }
}
