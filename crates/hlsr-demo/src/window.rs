//! Последовательное чтение отрезка файла окнами по 1 МБ (`Reader` из `goldsrc-demo.js`):
//! память не растёт с размером демки.

use std::io::{self, Read};

const CHUNK: usize = 1 << 20;

pub(crate) struct Window<R> {
    inner: R,
    /// Сколько байт отрезка ещё не прочитано из `inner`.
    left: u64,
    buffer: Vec<u8>,
    at: usize,
}

impl<R: Read> Window<R> {
    /// Отрезок длиной `length` с текущей позиции `inner`.
    pub(crate) fn new(inner: R, length: u64) -> Window<R> {
        Window {
            inner,
            left: length,
            buffer: Vec::new(),
            at: 0,
        }
    }

    fn rest(&self) -> usize {
        self.buffer.len() - self.at
    }

    fn available(&self) -> u64 {
        self.rest() as u64 + self.left
    }

    /// `n` байт подряд в памяти; `false` — до конца отрезка их нет.
    pub(crate) fn need(&mut self, n: usize) -> io::Result<bool> {
        if self.rest() >= n {
            return Ok(true);
        }
        if self.available() < n as u64 {
            return Ok(false);
        }
        self.buffer.drain(..self.at);
        self.at = 0;
        let want = (n - self.buffer.len())
            .max(CHUNK)
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let old = self.buffer.len();
        self.buffer.resize(old + want, 0);
        self.inner.read_exact(&mut self.buffer[old..])?;
        self.left -= want as u64;
        Ok(true)
    }

    /// Следующий байт; только после `need(1)`.
    pub(crate) fn peek(&self) -> u8 {
        self.buffer[self.at]
    }

    /// `n` байт; только после `need(n)`.
    pub(crate) fn take(&mut self, n: usize) -> &[u8] {
        let out = &self.buffer[self.at..self.at + n];
        self.at += n;
        out
    }

    /// Пропустить `n` байт, не держа их в памяти; `false` — до конца отрезка их нет.
    pub(crate) fn skip(&mut self, n: u64) -> io::Result<bool> {
        if self.available() < n {
            return Ok(false);
        }
        let buffered = (self.rest() as u64).min(n);
        self.at += buffered as usize;
        let unread = n - buffered;
        if unread > 0 {
            let copied = io::copy(&mut (&mut self.inner).take(unread), &mut io::sink())?;
            if copied < unread {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.left -= unread;
        }
        Ok(true)
    }
}
