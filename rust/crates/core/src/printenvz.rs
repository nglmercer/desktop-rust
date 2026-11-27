//! Environment printer protocol used when capturing a login shell's environment.
use std::ffi::OsString;
use std::io::{self, Write};

#[cfg(unix)]
fn bytes(value: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(value.as_bytes())
}

#[cfg(not(unix))]
fn bytes(value: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Owned(value.to_string_lossy().into_owned().into_bytes())
}

pub fn write_environment(
    mut output: impl Write,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<()> {
    output.write_all(b"--printenvz--begin\n")?;
    for (name, value) in environment {
        if name.is_empty() {
            continue;
        }
        output.write_all(&bytes(&name))?;
        output.write_all(b"=")?;
        output.write_all(&bytes(&value))?;
        output.write_all(b"\0")?;
    }
    output.write_all(b"\n--printenvz--end\n")
}

pub fn print_environment() -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    write_environment(&mut output, std::env::vars_os())?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_protocol_and_special_values() {
        let mut output = Vec::new();
        write_environment(
            &mut output,
            [
                (OsString::from("EMPTY"), OsString::new()),
                (OsString::from("WEIRD"), OsString::from("a=b\n☃")),
            ],
        )
        .unwrap();
        assert_eq!(
            output,
            b"--printenvz--begin\nEMPTY=\0WEIRD=a=b\n\xe2\x98\x83\0\n--printenvz--end\n"
        );
    }
}
