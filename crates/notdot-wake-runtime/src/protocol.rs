//! Versioned private pipes. Audio is bounded and ephemeral; responses contain no text.
use std::io::{self, Read, Write};
pub const MAGIC: &[u8; 8] = b"NDWK0001";
pub const READY: &[u8; 8] = b"NDWKRDY1";
pub const MAX_SAMPLES: usize = 16000 * 6 + 512;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid wake helper frame")
}
pub fn write_hello(mut out: impl Write, phrase: &str) -> io::Result<()> {
    if phrase.is_empty()
        || phrase.len() > 84
        || !phrase
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-' || c == '\'')
    {
        return Err(invalid());
    }
    out.write_all(MAGIC)?;
    out.write_all(&(phrase.len() as u16).to_le_bytes())?;
    out.write_all(phrase.as_bytes())?;
    out.flush()
}
pub fn read_hello(mut input: impl Read) -> io::Result<String> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid());
    }
    let mut size = [0; 2];
    input.read_exact(&mut size)?;
    let size = usize::from(u16::from_le_bytes(size));
    if size == 0 || size > 84 {
        return Err(invalid());
    }
    let mut text = vec![0; size];
    input.read_exact(&mut text)?;
    let text = String::from_utf8(text).map_err(|_| invalid())?;
    let mut checked = Vec::new();
    write_hello(&mut checked, &text)?;
    Ok(text)
}
pub fn encode_request(id: u64, samples: &[f32]) -> io::Result<Vec<u8>> {
    if samples.is_empty()
        || samples.len() > MAX_SAMPLES
        || samples.iter().any(|v| !v.is_finite() || v.abs() > 4.)
    {
        return Err(invalid());
    }
    let mut out = Vec::with_capacity(13 + samples.len() * 4);
    out.push(1);
    out.extend(id.to_le_bytes());
    out.extend((samples.len() as u32).to_le_bytes());
    for value in samples {
        out.extend(value.to_le_bytes());
    }
    Ok(out)
}
pub fn read_request(mut input: impl Read) -> io::Result<Option<(u64, Vec<f32>)>> {
    let mut tag = [0];
    match input.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    };
    if tag[0] != 1 {
        return Err(invalid());
    }
    let mut id = [0; 8];
    input.read_exact(&mut id)?;
    let mut n = [0; 4];
    input.read_exact(&mut n)?;
    let n = u32::from_le_bytes(n) as usize;
    if n == 0 || n > MAX_SAMPLES {
        return Err(invalid());
    }
    let mut bytes = vec![0; n * 4];
    input.read_exact(&mut bytes)?;
    let samples: Vec<_> = bytes
        .chunks_exact(4)
        .map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]))
        .collect();
    if samples.iter().any(|v| !v.is_finite() || v.abs() > 4.) {
        return Err(invalid());
    }
    Ok(Some((u64::from_le_bytes(id), samples)))
}
pub fn response(id: u64, result: Result<bool, ()>) -> [u8; 9] {
    let mut out = [0; 9];
    out[..8].copy_from_slice(&id.to_le_bytes());
    out[8] = match result {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(()) => 2,
    };
    out
}
pub fn parse_response(bytes: &[u8; 9], expected: u64) -> io::Result<bool> {
    if bytes[..8] != expected.to_le_bytes() {
        return Err(invalid());
    }
    match bytes[8] {
        0 => Ok(false),
        1 => Ok(true),
        2 => Err(io::Error::other("Wake helper could not finish")),
        _ => Err(invalid()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_round_trip_and_rejection_boundaries() {
        let mut hello = Vec::new();
        write_hello(&mut hello, "Hey Ava").unwrap();
        assert_eq!(read_hello(&hello[..]).unwrap(), "Hey Ava");
        assert!(write_hello(Vec::new(), &"a".repeat(85)).is_err());
        assert!(read_hello(&b"bad magic"[..]).is_err());
        let packet = encode_request(7, &[0., 0.2, -0.5]).unwrap();
        let (id, audio) = read_request(&packet[..]).unwrap().unwrap();
        assert_eq!(id, 7);
        assert_eq!(audio, [0., 0.2, -0.5]);
        assert!(read_request(&[][..]).unwrap().is_none());
        assert!(read_request(&packet[..packet.len() - 1]).is_err());
        assert!(encode_request(0, &[f32::NAN]).is_err());
        assert!(encode_request(0, &vec![0.; MAX_SAMPLES + 1]).is_err());
        let mut oversized = vec![1];
        oversized.extend(0u64.to_le_bytes());
        oversized.extend(u32::MAX.to_le_bytes());
        assert!(read_request(&oversized[..]).is_err());
        let mut nonfinite = packet.clone();
        nonfinite[13..17].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(read_request(&nonfinite[..]).is_err());
        let mut invalid_tag = packet.clone();
        invalid_tag[0] = 99;
        assert!(read_request(&invalid_tag[..]).is_err());
        let mut invalid_status = response(7, Ok(true));
        invalid_status[8] = 99;
        assert!(parse_response(&invalid_status, 7).is_err());
        assert!(parse_response(&response(6, Ok(true)), 7).is_err());
        assert!(parse_response(&response(7, Err(())), 7).is_err());
        assert!(parse_response(&response(7, Ok(true)), 7).unwrap());
    }
}
