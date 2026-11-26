//! The base protocol of LSP: a `Content-Length` header, a blank line, then a JSON body.

use std::io::{self, BufRead, Write};

pub fn write_message(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

/// The next message body, or `None` when the stream ended cleanly between messages.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return if length.is_none() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        // other headers (Content-Type) carry nothing we need
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length =
                Some(value.trim().parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
                })?);
        }
    }
    let length = length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "message without Content-Length")
    })?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_survives_a_round_trip() {
        let mut wire = Vec::new();
        write_message(&mut wire, b"{\"a\":1}").unwrap();
        write_message(&mut wire, "{\"é\":2}".as_bytes()).unwrap();
        let mut reader = io::BufReader::new(wire.as_slice());
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(
            read_message(&mut reader).unwrap().unwrap(),
            "{\"é\":2}".as_bytes()
        );
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn other_headers_and_case_are_tolerated() {
        let wire = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}";
        let mut reader = io::BufReader::new(&wire[..]);
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn broken_streams_are_errors() {
        let truncated = b"Content-Length: 10\r\n\r\n{}";
        assert!(read_message(&mut io::BufReader::new(&truncated[..])).is_err());
        let no_length = b"Content-Type: x\r\n\r\n{}";
        assert!(read_message(&mut io::BufReader::new(&no_length[..])).is_err());
        let bad = b"Content-Length: abc\r\n\r\n";
        assert!(read_message(&mut io::BufReader::new(&bad[..])).is_err());
        let cut_header = b"Content-Length: 2\r\n";
        assert!(read_message(&mut io::BufReader::new(&cut_header[..])).is_err());
    }
}
