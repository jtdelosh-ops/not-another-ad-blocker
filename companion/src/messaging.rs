use std::io::{self, ErrorKind, Read, Write};

pub const MAX_INCOMING_BYTES: usize = 256 * 1024;
pub const MAX_OUTGOING_BYTES: usize = 1024 * 1024;

pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 4];
    match input.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    input.read_exact(&mut header[1..]).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            io::Error::new(ErrorKind::UnexpectedEof, "truncated native message header")
        } else {
            error
        }
    })?;
    let size = u32::from_ne_bytes(header) as usize;
    if size > MAX_INCOMING_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "native message exceeds 256 KiB input limit",
        ));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            io::Error::new(ErrorKind::UnexpectedEof, "truncated native message body")
        } else {
            error
        }
    })?;
    Ok(Some(bytes))
}

pub fn write_frame(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_OUTGOING_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "native response exceeds 1 MiB output limit",
        ));
    }
    output.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    output.write_all(bytes)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn multiple_frames_round_trip_with_native_endian_lengths() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, b"one").unwrap();
        write_frame(&mut bytes, b"two").unwrap();
        assert_eq!(&bytes[..4], &3_u32.to_ne_bytes());
        let mut input = Cursor::new(bytes);
        assert_eq!(read_frame(&mut input).unwrap(), Some(b"one".to_vec()));
        assert_eq!(read_frame(&mut input).unwrap(), Some(b"two".to_vec()));
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn eof_is_only_clean_between_frames() {
        assert_eq!(read_frame(&mut &[][..]).unwrap(), None);
        for count in 1..4 {
            assert_eq!(
                read_frame(&mut &[1, 0, 0][..count]).unwrap_err().kind(),
                ErrorKind::UnexpectedEof
            );
        }
        let mut truncated = 8_u32.to_ne_bytes().to_vec();
        truncated.extend_from_slice(b"short");
        assert_eq!(
            read_frame(&mut &truncated[..]).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rejects_oversized_input_without_reading_a_body() {
        let bytes = (MAX_INCOMING_BYTES as u32 + 1).to_ne_bytes();
        assert_eq!(
            read_frame(&mut &bytes[..]).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn accepts_exact_input_bound_and_zero_length_frame() {
        let mut bytes = (MAX_INCOMING_BYTES as u32).to_ne_bytes().to_vec();
        bytes.resize(MAX_INCOMING_BYTES + 4, 0);
        assert_eq!(
            read_frame(&mut &bytes[..]).unwrap().unwrap().len(),
            MAX_INCOMING_BYTES
        );
        assert_eq!(
            read_frame(&mut &0_u32.to_ne_bytes()[..]).unwrap(),
            Some(vec![])
        );
    }

    #[test]
    fn refuses_oversized_output_without_writing_partial_header() {
        let mut output = vec![];
        let error = write_frame(&mut output, &vec![0; MAX_OUTGOING_BYTES + 1]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(output.is_empty());
    }

    #[test]
    fn partial_reads_and_interrupted_reads_are_supported() {
        struct SlowReader {
            bytes: Cursor<Vec<u8>>,
            interrupt: bool,
        }
        impl Read for SlowReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.interrupt {
                    self.interrupt = false;
                    return Err(io::Error::new(ErrorKind::Interrupted, "retry"));
                }
                self.interrupt = true;
                let len = buffer.len().min(1);
                self.bytes.read(&mut buffer[..len])
            }
        }
        let mut bytes = vec![];
        write_frame(&mut bytes, b"hello").unwrap();
        let mut reader = SlowReader {
            bytes: Cursor::new(bytes),
            interrupt: true,
        };
        assert_eq!(read_frame(&mut reader).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }
}
