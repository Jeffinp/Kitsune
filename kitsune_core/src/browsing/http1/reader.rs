//! [`ResponseReader`]: one HTTP/1.x response read incrementally from a connection.

use super::*;

/// How the body of a response is delimited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Framing {
    /// No body (`HEAD`, `204`, `304`, `Content-Length: 0`).
    None,
    /// Exactly this many bytes.
    Length(u64),
    /// `Transfer-Encoding: chunked`.
    Chunked,
    /// Everything until the server closes the connection.
    UntilClose,
}

/// What the header block said about the rest of the exchange.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HeadInfo {
    pub status: u16,
    /// The server allows another request on this connection.
    pub keep_alive: bool,
    pub framing: Framing,
}

/// A piece of the response handed on by [`ResponseReader::feed`].
#[derive(Debug, PartialEq, Eq)]
pub enum Part<'a> {
    /// The final header block (status line to the blank line, inclusive), once.
    Head(&'a [u8]),
    /// Body bytes, with any chunk framing removed.
    Body(&'a [u8]),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chunk {
    /// Reading a size line: the value so far, how many hex digits, and whether an extension (or
    /// trailing blanks) started.
    Size { n: u64, digits: u32, ext: bool },
    /// Inside a chunk, this many bytes to go.
    Data(u64),
    /// After a chunk's data: its CRLF.
    DataEnd { cr: bool },
    /// After the last chunk: trailer lines until an empty one (`len` = bytes on this line).
    Trailer { len: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Head,
    Length(u64),
    Chunked(Chunk),
    UntilClose,
    Done,
}

/// Reads one response from a connection, fed with whatever arrives.
pub struct ResponseReader {
    state: State,
    head: Vec<u8>,
    info: Option<HeadInfo>,
    head_request: bool,
    /// Interim (1xx) responses skipped so far.
    interim: u32,
}

impl ResponseReader {
    /// A reader for the answer to a request; `head_request` when the request was a `HEAD` (no body
    /// follows whatever the headers say).
    pub fn new(head_request: bool) -> ResponseReader {
        ResponseReader {
            state: State::Head,
            head: Vec::new(),
            info: None,
            head_request,
            interim: 0,
        }
    }

    /// What the final header block said, once it is in.
    pub fn info(&self) -> Option<HeadInfo> {
        self.info
    }

    /// The final header block, once it is in.
    pub fn head(&self) -> Option<&[u8]> {
        self.info.map(|_| self.head.as_slice())
    }

    /// The whole response was read.
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// The response is complete and the connection may carry another request.
    pub fn reusable(&self) -> bool {
        self.is_done()
            && self
                .info
                .is_some_and(|i| i.keep_alive && i.framing != Framing::UntilClose)
    }

    /// Have any bytes of the response arrived?
    pub fn started(&self) -> bool {
        !self.head.is_empty() || self.info.is_some()
    }

    /// Take `input` (the next bytes from the connection); `f` receives the header block once and then
    /// the body. Returns how many bytes belong to this response (less than `input.len()` only when
    /// it ended inside `input`).
    pub fn feed(&mut self, input: &[u8], f: &mut dyn FnMut(Part<'_>)) -> Result<usize, HttpError> {
        let mut at = 0;
        while at < input.len() {
            let rest = &input[at..];
            match self.state {
                State::Done => break,
                State::Head => at += self.feed_head(rest, f)?,
                State::Length(left) => {
                    let n = rest.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                    f(Part::Body(&rest[..n]));
                    at += n;
                    let left = left - n as u64;
                    self.state = if left == 0 {
                        State::Done
                    } else {
                        State::Length(left)
                    };
                }
                State::UntilClose => {
                    f(Part::Body(rest));
                    at = input.len();
                }
                State::Chunked(c) => at += self.feed_chunked(c, rest, f)?,
            }
        }
        Ok(at)
    }

    /// The connection closed: fine when the body runs until the close (or the response is complete).
    pub fn eof(&mut self) -> Result<(), HttpError> {
        match self.state {
            State::Done => Ok(()),
            State::UntilClose => {
                self.state = State::Done;
                Ok(())
            }
            _ => Err(HttpError::Incomplete),
        }
    }

    /// Gather the header block; returns the bytes of `rest` used.
    fn feed_head(&mut self, rest: &[u8], f: &mut dyn FnMut(Part<'_>)) -> Result<usize, HttpError> {
        let before = self.head.len();
        let from = before.saturating_sub(3);
        self.head.extend_from_slice(rest);
        let end = find_blank_line(&self.head[from..]).map(|e| from + e);
        let Some(end) = end else {
            if self.head.len() > MAX_HEAD {
                return Err(HttpError::HeadTooLarge);
            }
            return Ok(rest.len());
        };
        if end > MAX_HEAD {
            return Err(HttpError::HeadTooLarge);
        }
        self.head.truncate(end);
        let used = end - before;
        let (status, _) = status_line(&self.head).ok_or(HttpError::BadStatus)?;
        if (100..200).contains(&status) && status != 101 {
            // An interim answer: the real one follows on the same connection.
            self.interim += 1;
            if self.interim > 8 {
                return Err(HttpError::BadStatus);
            }
            self.head.clear();
            return Ok(used);
        }
        let info = self.head_info(status)?;
        self.info = Some(info);
        self.state = match info.framing {
            Framing::None | Framing::Length(0) => State::Done,
            Framing::Length(n) => State::Length(n),
            Framing::Chunked => State::Chunked(Chunk::Size {
                n: 0,
                digits: 0,
                ext: false,
            }),
            Framing::UntilClose => State::UntilClose,
        };
        f(Part::Head(&self.head));
        Ok(used)
    }

    fn head_info(&self, status: u16) -> Result<HeadInfo, HttpError> {
        let head = &self.head;
        let v11 = head.starts_with(b"HTTP/1.1");
        let conn = header_joined(head, b"connection").unwrap_or_default();
        let mut keep_alive = if v11 {
            !has_token(conn.as_bytes(), b"close")
        } else {
            has_token(conn.as_bytes(), b"keep-alive")
        };
        let no_body = self.head_request || status == 204 || status == 304 || status == 101;
        let framing = if no_body {
            if status == 101 {
                keep_alive = false;
            }
            Framing::None
        } else if header_joined(head, b"transfer-encoding")
            .is_some_and(|te| last_token_is(te.as_bytes(), b"chunked"))
        {
            Framing::Chunked
        } else {
            let mut len: Option<u64> = None;
            for (n, v) in header_lines(head) {
                if !n.eq_ignore_ascii_case(b"content-length") {
                    continue;
                }
                // `Content-Length: 5, 5` is allowed (the same value repeated).
                for part in v.split(|&b| b == b',') {
                    let x = parse_dec(part.trim_ascii()).ok_or(HttpError::BadLength)?;
                    if len.is_some_and(|l| l != x) {
                        return Err(HttpError::BadLength);
                    }
                    len = Some(x);
                }
            }
            match len {
                Some(0) => Framing::None,
                Some(n) => Framing::Length(n),
                None => {
                    keep_alive = false;
                    Framing::UntilClose
                }
            }
        };
        Ok(HeadInfo {
            status,
            keep_alive,
            framing,
        })
    }

    /// One step of the chunked decoder; returns the bytes of `rest` used.
    fn feed_chunked(
        &mut self,
        c: Chunk,
        rest: &[u8],
        f: &mut dyn FnMut(Part<'_>),
    ) -> Result<usize, HttpError> {
        match c {
            Chunk::Data(left) => {
                let n = rest.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                f(Part::Body(&rest[..n]));
                let left = left - n as u64;
                self.state = State::Chunked(if left == 0 {
                    Chunk::DataEnd { cr: false }
                } else {
                    Chunk::Data(left)
                });
                Ok(n)
            }
            Chunk::DataEnd { cr } => {
                self.state = match (rest[0], cr) {
                    (b'\r', false) => State::Chunked(Chunk::DataEnd { cr: true }),
                    (b'\n', _) => State::Chunked(Chunk::Size {
                        n: 0,
                        digits: 0,
                        ext: false,
                    }),
                    _ => return Err(HttpError::BadChunk),
                };
                Ok(1)
            }
            Chunk::Size { n, digits, ext } => {
                let b = rest[0];
                let next = match b {
                    b'\n' => {
                        if digits == 0 {
                            return Err(HttpError::BadChunk);
                        }
                        if n == 0 {
                            Chunk::Trailer { len: 0 }
                        } else {
                            Chunk::Data(n)
                        }
                    }
                    b'\r' => Chunk::Size { n, digits, ext },
                    _ if ext => Chunk::Size { n, digits, ext },
                    b';' | b' ' | b'\t' => Chunk::Size {
                        n,
                        digits,
                        ext: true,
                    },
                    _ => {
                        let v = (b as char).to_digit(16).ok_or(HttpError::BadChunk)?;
                        if digits >= 15 {
                            return Err(HttpError::BadChunk);
                        }
                        Chunk::Size {
                            n: n * 16 + u64::from(v),
                            digits: digits + 1,
                            ext,
                        }
                    }
                };
                self.state = State::Chunked(next);
                Ok(1)
            }
            Chunk::Trailer { len } => {
                self.state = match rest[0] {
                    b'\n' if len == 0 => State::Done,
                    b'\n' => State::Chunked(Chunk::Trailer { len: 0 }),
                    b'\r' => State::Chunked(Chunk::Trailer { len }),
                    _ if len > MAX_HEAD => return Err(HttpError::HeadTooLarge),
                    _ => State::Chunked(Chunk::Trailer { len: len + 1 }),
                };
                Ok(1)
            }
        }
    }
}

/// The end of the first blank line (`\r\n\r\n` or `\n\n`) in `b`.
fn find_blank_line(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            match (b.get(i + 1), b.get(i + 2)) {
                (Some(b'\n'), _) => return Some(i + 2),
                (Some(b'\r'), Some(b'\n')) => return Some(i + 3),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Is the last entry of the comma list `v` equal to `token` (any case)?
fn last_token_is(v: &[u8], token: &[u8]) -> bool {
    v.split(|&b| b == b',')
        .rev()
        .find(|t| !t.trim_ascii().is_empty())
        .is_some_and(|t| t.trim_ascii().eq_ignore_ascii_case(token))
}
