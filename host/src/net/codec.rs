use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("message truncated")]
    Truncated,
    #[error("{0} trailing bytes after message")]
    Trailing(usize),
    #[error("string is not valid UTF-8")]
    BadUtf8,
    #[error("bad magic or version")]
    BadHeader,
    #[error("unexpected kind 0x{0:02x}")]
    BadKind(u8),
    #[error("empty message")]
    Empty,
    #[error("field out of range: {0}")]
    OutOfRange(&'static str),
}

pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn str8(&mut self) -> Result<String, DecodeError> {
        let n = self.u8()? as usize;
        Self::utf8(self.bytes(n)?)
    }

    pub fn str16(&mut self) -> Result<String, DecodeError> {
        let n = self.u16()? as usize;
        Self::utf8(self.bytes(n)?)
    }

    pub fn rest(&mut self) -> Vec<u8> {
        let out = self.buf.to_vec();
        self.buf = &[];
        out
    }

    pub fn finish(self) -> Result<(), DecodeError> {
        if self.buf.is_empty() { Ok(()) } else { Err(DecodeError::Trailing(self.buf.len())) }
    }

    fn utf8(b: &[u8]) -> Result<String, DecodeError> {
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
}

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    /// Strings longer than the length prefix allows are truncated at a UTF-8 boundary.
    pub fn str8(&mut self, s: &str) -> &mut Self {
        let s = truncate_utf8(s, u8::MAX as usize);
        self.u8(s.len() as u8).bytes(s.as_bytes())
    }

    pub fn str16(&mut self, s: &str) -> &mut Self {
        let s = truncate_utf8(s, u16::MAX as usize);
        self.u16(s.len() as u16).bytes(s.as_bytes())
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_primitives() {
        let mut w = Writer::new();
        w.u8(1).u16(0x0203).u32(0x0405_0607).u64(0x0809_0a0b_0c0d_0e0f).str8("hé").str16("xin chào");
        let bytes = w.into_vec();
        assert_eq!(&bytes[1..3], &[0x03, 0x02], "little-endian");
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0203);
        assert_eq!(r.u32().unwrap(), 0x0405_0607);
        assert_eq!(r.u64().unwrap(), 0x0809_0a0b_0c0d_0e0f);
        assert_eq!(r.str8().unwrap(), "hé");
        assert_eq!(r.str16().unwrap(), "xin chào");
        r.finish().unwrap();
    }

    #[test]
    fn rejects_truncated_and_trailing() {
        assert_eq!(Reader::new(&[1]).u16(), Err(DecodeError::Truncated));
        assert_eq!(Reader::new(&[5, b'a']).str8(), Err(DecodeError::Truncated));
        assert_eq!(Reader::new(&[1, 0xff]).str8(), Err(DecodeError::BadUtf8));
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(DecodeError::Trailing(1)));
    }

    #[test]
    fn truncates_on_char_boundary() {
        let s = "a".repeat(254) + "ộ";
        let mut w = Writer::new();
        w.str8(&s);
        let bytes = w.into_vec();
        assert_eq!(bytes[0], 254);
        assert_eq!(Reader::new(&bytes).str8().unwrap(), "a".repeat(254));
    }
}
