use std::path::PathBuf;

/// Format one shell paste without changing the supplied path spelling.
pub(super) fn format_paths(
    paths: &[PathBuf],
    capacity: usize,
) -> Result<String, &'static str> {
    if paths.is_empty() {
        return Err("Drop contains no paths");
    }
    let escape = |character: char| {
        character.is_ascii()
            && !character.is_ascii_alphanumeric()
            && !matches!(
                character,
                '/' | '.' | '_' | '-' | ':' | ',' | '+' | '%'
            )
    };
    let mut length = 0usize;
    for path in paths {
        if !path.is_absolute() {
            return Err("Dropped paths must be absolute");
        }
        let text = path.to_str().ok_or("Dropped paths must be UTF-8")?;
        for character in text.chars() {
            if character.is_control() {
                return Err("Dropped paths cannot contain control characters");
            }
            length = length.saturating_add(
                character.len_utf8() + usize::from(escape(character)),
            );
            if length > capacity {
                return Err("Dropped paths exceed paste capacity");
            }
        }
        if length == capacity {
            return Err("Dropped paths exceed paste capacity");
        }
        length += 1;
    }
    // Validate before allocation and reserve the exact admitted byte count.
    let mut output = String::with_capacity(length);
    for path in paths {
        let text = path.to_str().ok_or("Dropped paths must be UTF-8")?;
        for character in text.chars() {
            if escape(character) {
                output.push('\\');
            }
            output.push(character);
        }
        output.push(' ');
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_paths_escape_shell_syntax_and_keep_symlink_spelling() {
        let paths = [
            PathBuf::from("/tmp/a b/'\"\\$`*?[];|&()<>{}!#~^=λ"),
            PathBuf::from("/tmp/link/../folder"),
        ];
        assert_eq!(
            format_paths(&paths, 1024).unwrap(),
            "/tmp/a\\ b/\\'\\\"\\\\\\$\\`\\*\\?\\[\\]\\;\\|\\&\\(\\)\\<\\>\\{\\}\\!\\#\\~\\^\\=λ /tmp/link/../folder "
        );
    }

    #[test]
    fn reject_whole_drop_for_invalid_or_oversized_item() {
        for value in [
            "relative",
            "/tmp/line\n",
            "/tmp/tab\t",
            "/tmp/esc\x1b",
            "/tmp/del\x7f",
        ] {
            assert!(
                format_paths(&["/valid".into(), value.into()], 1024).is_err()
            );
        }
        assert!(format_paths(&[], 1024).is_err());
        assert_eq!(format_paths(&["/a".into()], 3).unwrap(), "/a ");
        assert!(format_paths(&["/a".into()], 2).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn rejects_non_utf8_without_lossy_conversion() {
        use std::os::unix::ffi::OsStringExt;
        assert!(
            format_paths(
                &[std::ffi::OsString::from_vec(vec![b'/', 255]).into()],
                1024
            )
            .is_err()
        );
    }
    #[test]
    #[cfg(unix)]
    fn escaped_paths_round_trip_through_bash_and_zsh_argument_parsers() {
        let paths = [
            PathBuf::from("/tmp/a b/'\"\\$`*?[];|&()<>{}!#~^=λ"),
            PathBuf::from("/tmp/$(printf INJECT)/link/../folder"),
        ];
        let formatted = format_paths(&paths, 1024).unwrap();
        let script = format!("set -- {formatted}; printf '%s\\0' \"$@\"");
        let expected: Vec<u8> = paths
            .iter()
            .flat_map(|path| {
                path.to_str().unwrap().as_bytes().iter().copied().chain([0])
            })
            .collect();
        for shell in ["bash", "zsh"] {
            let output = std::process::Command::new(shell)
                .args(["-f", "-c", &script])
                .output()
                .unwrap();
            assert!(output.status.success(), "{shell}: {:?}", output.stderr);
            assert_eq!(output.stdout, expected, "{shell}");
        }
    }
}
