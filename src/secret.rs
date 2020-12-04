use std::io::Write as _;
use std::os::unix::io::FromRawFd as _;

pub struct Passphrase(pub(crate) SecBuf<char>);

impl Passphrase {
    pub fn write_stdout(&self) -> std::io::Result<()> {
        // Avoid line buffering
        // This is unsafe because from_raw_fd assumes it will be the only one using this file descriptor.
        // So ensure no logging during its lifetime.
        // TODO any more guarantees that this is safe?
        let mut stdout = unsafe { std::fs::File::from_raw_fd(1) };

        // Keep the encoded values in secure buffer too
        // A buffer of length four is large enough to encode any char.
        // Add space for newline
        let mut buf: SecBuf<u8> = SecBuf::new(vec![0; 4 * self.0.len + 1]);
        for c in self.0.unsecure() {
            let ret = c.encode_utf8(&mut buf.buf.unsecure_mut()[buf.len..]);
            buf.len += ret.len();
        }

        buf.buf.unsecure_mut()[buf.len] = b'\n';
        buf.len += 1;

        let ret = stdout.write_all(buf.unsecure());

        // avoid closing stdout
        std::mem::forget(stdout);

        ret
    }
}

pub struct SecBuf<T: Copy> {
    pub(crate) buf: secstr::SecVec<T>,
    pub(crate) len: usize,
}

impl<T: Copy> SecBuf<T> {
    pub fn new(buf: Vec<T>) -> Self {
        Self {
            buf: secstr::SecVec::new(buf),
            len: 0,
        }
    }

    pub fn unsecure(&self) -> &[T] {
        &self.buf[0..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret() {
        let mut buf = SecBuf::new(vec!['X'; 20]);
        buf.buf.unsecure_mut()[0] = 'a';
        buf.len = 1;
        assert_eq!(buf.unsecure(), ['a']);
        buf.len = 2;
        assert_eq!(buf.unsecure(), ['a', 'X']);
    }
}
