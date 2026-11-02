use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn read_json_line<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte)? {
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed the socket",
                ));
            }
            _ if byte[0] == b'\n' => {
                if buf.is_empty() {
                    continue;
                }
                break;
            }
            _ => buf.push(byte[0]),
        }
    }
    serde_json::from_slice(&buf).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub fn write_json_line<T: Serialize>(stream: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line =
        serde_json::to_vec(msg).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    line.push(b'\n');
    stream.write_all(&line)
}

pub fn read_json_lines<T: DeserializeOwned>(
    stream: &mut impl Read,
    buf: &mut Vec<u8>,
) -> io::Result<Vec<T>> {
    let mut tmp = [0u8; 4096];
    let mut eof = false;
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }

    let mut messages = Vec::new();
    while let Some(pos) = buf.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        let line = &line[..line.len() - 1];
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<T>(line) {
            Ok(msg) => messages.push(msg),
            Err(err) => {
                log_invalid(line, err);
            }
        }
    }
    reclaim_read_buf(buf);
    if eof && messages.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "peer closed the socket",
        ));
    }
    Ok(messages)
}

/// Drop spare capacity after a spike. Keep only the leftover partial line.
fn reclaim_read_buf(buf: &mut Vec<u8>) {
    if buf.capacity() > buf.len() {
        buf.shrink_to_fit();
    }
}

fn log_invalid(line: &[u8], err: serde_json::Error) {
    let preview = String::from_utf8_lossy(line);
    eprintln!("vile-protocol: skipped invalid JSON ({err}): {preview}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_json_lines_drops_spare_capacity() {
        let mut data = Vec::new();
        for i in 0..8_000 {
            data.extend_from_slice(format!("{{\"n\":{i}}}\n").as_bytes());
        }
        let mut cursor = Cursor::new(data);
        let mut buf = Vec::new();
        let msgs: Vec<serde_json::Value> = read_json_lines(&mut cursor, &mut buf).unwrap();
        assert_eq!(msgs.len(), 8_000);
        assert!(buf.is_empty());
        assert!(
            buf.capacity() < 1024,
            "leftover capacity {}",
            buf.capacity()
        );
    }

    #[test]
    fn keeps_partial_line_without_spare() {
        let data = b"{\"a\":1}\n{\"b\":";
        let mut cursor = Cursor::new(&data[..]);
        let mut buf = Vec::new();
        let msgs: Vec<serde_json::Value> = read_json_lines(&mut cursor, &mut buf).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(buf.as_slice(), b"{\"b\":");
        assert!(
            buf.capacity() <= buf.len() + 64,
            "leftover capacity {}",
            buf.capacity()
        );
    }
}
