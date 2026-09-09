//! Bounded native file-URI decoding without path normalization.
use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

pub(super) const MAX_URI_LIST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub(super) enum Transfer {
    #[default]
    Idle,
    Requested(u32),
    Ready(u32),
    Rejected,
}

impl Transfer {
    pub(super) fn requestor(&self) -> Option<u32> {
        match *self {
            Self::Requested(window) | Self::Ready(window) => Some(window),
            Self::Idle | Self::Rejected => None,
        }
    }

    pub(super) fn accepts_reply(&self, requestor: u32) -> bool {
        matches!(*self, Self::Requested(window) if window == requestor)
    }

    pub(super) fn ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

pub(super) fn parse_uri_list(bytes: &[u8], bytes_after: u32) -> Option<Vec<PathBuf>> {
    if bytes_after != 0 || bytes.len() > MAX_URI_LIST_BYTES {
        return None;
    }
    let list = std::str::from_utf8(bytes).ok()?;
    let paths = list
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(parse_file_uri)
        .collect::<Option<Vec<_>>>()?;
    (!paths.is_empty()).then_some(paths)
}

fn parse_file_uri(uri: &str) -> Option<PathBuf> {
    // Url::to_file_path normalizes dot segments. Decode the original path so
    // /symlink/../name keeps the exact meaning supplied by the drag source.
    if !uri.get(..5)?.eq_ignore_ascii_case("file:")
        || uri.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '?' | '#' | '\\')
        })
    {
        return None;
    }
    let mut path = &uri[5..];
    if let Some(authority_and_path) = path.strip_prefix("//") {
        let slash = authority_and_path.find('/')?;
        let authority = &authority_and_path[..slash];
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            return None;
        }
        path = &authority_and_path[slash..];
    }
    if !path.starts_with('/') {
        return None;
    }
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            decoded.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            decoded.push(byte);
        }
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}
