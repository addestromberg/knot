//! Bound stalls on portable blocking pipes without buffering whole objects.
use std::{
    io::{self, Cursor, Read, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

const CHUNK: usize = 64 * 1024;

fn stalled() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "synchronization I/O progress timeout",
    )
}

// Workers own only transport handles, never a repository or its lock. If an OS
// pipe cannot be interrupted, a timed-out CLI can still drop its backend and
// exit. SSH callers kill their child, unblocking the workers' pipe operations.
pub(crate) struct ProgressReader {
    incoming: Receiver<io::Result<Vec<u8>>>,
    buffered: Cursor<Vec<u8>>,
    timeout: Duration,
    ended: bool,
}
impl ProgressReader {
    pub fn new(mut input: impl Read + Send + 'static, timeout: Duration) -> io::Result<Self> {
        let (sender, incoming) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("knot-sync-input".into())
            .spawn(move || {
                loop {
                    let mut bytes = vec![0; CHUNK];
                    let result = input.read(&mut bytes).map(|len| {
                        bytes.truncate(len);
                        bytes
                    });
                    let ended = match &result {
                        Ok(bytes) => bytes.is_empty(),
                        Err(error) => error.kind() != io::ErrorKind::Interrupted,
                    };
                    if sender.send(result).is_err() || ended {
                        break;
                    }
                }
            })?;
        Ok(Self {
            incoming,
            buffered: Cursor::new(Vec::new()),
            timeout,
            ended: false,
        })
    }
}
impl Read for ProgressReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.buffered.position() < self.buffered.get_ref().len() as u64 {
            return self.buffered.read(bytes);
        }
        if self.ended {
            return Ok(0);
        }
        match self.incoming.recv_timeout(self.timeout) {
            Ok(Ok(chunk)) => {
                self.ended = chunk.is_empty();
                self.buffered = Cursor::new(chunk);
                self.buffered.read(bytes)
            }
            Ok(Err(error)) => {
                self.ended = error.kind() != io::ErrorKind::Interrupted;
                Err(error)
            }
            Err(RecvTimeoutError::Timeout) => Err(stalled()),
            Err(RecvTimeoutError::Disconnected) => {
                self.ended = true;
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "synchronization reader stopped",
                ))
            }
        }
    }
}

enum Output {
    Bytes(Vec<u8>),
    Flush,
}
pub(crate) struct ProgressWriter {
    outgoing: Sender<Output>,
    completed: Receiver<io::Result<usize>>,
    timeout: Duration,
    failed: bool,
}
impl ProgressWriter {
    pub fn new(mut output: impl Write + Send + 'static, timeout: Duration) -> io::Result<Self> {
        // There is only one outstanding bounded chunk: each send waits for its
        // completion before the calling thread can enqueue another operation.
        let (outgoing, requests) = mpsc::channel();
        let (finished, completed) = mpsc::channel();
        thread::Builder::new()
            .name("knot-sync-output".into())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let result = match request {
                        Output::Bytes(bytes) => output.write(&bytes),
                        Output::Flush => output.flush().map(|()| 0),
                    };
                    let failed = result
                        .as_ref()
                        .is_err_and(|error| error.kind() != io::ErrorKind::Interrupted);
                    if finished.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            outgoing,
            completed,
            timeout,
            failed: false,
        })
    }
    fn perform(&mut self, request: Output) -> io::Result<usize> {
        if self.failed {
            return Err(stalled());
        }
        let result = (|| {
            self.outgoing.send(request).map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "synchronization writer stopped")
            })?;
            match self.completed.recv_timeout(self.timeout) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => Err(stalled()),
                Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "synchronization writer stopped",
                )),
            }
        })();
        // Never enqueue another response after timeout: a late completion from
        // the old write must not be mistaken for the next write's completion.
        self.failed = result
            .as_ref()
            .is_err_and(|error| error.kind() != io::ErrorKind::Interrupted);
        result
    }
}
impl Write for ProgressWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.perform(Output::Bytes(bytes[..bytes.len().min(CHUNK)].to_vec()))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.perform(Output::Flush).map(|_| ())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    struct Gate(Receiver<()>);
    impl Read for Gate {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            let _ = self.0.recv();
            Ok(0)
        }
    }
    impl Write for Gate {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let _ = self.0.recv();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            let _ = self.0.recv();
            Ok(())
        }
    }
    #[test]
    fn stalled_reads_writes_and_flushes_have_deadlines() {
        let timeout = Duration::from_millis(50);
        let (release, gate) = mpsc::channel();
        let mut reader = ProgressReader::new(Gate(gate), timeout).unwrap();
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        drop(release);
        for flush in [false, true] {
            let (release, gate) = mpsc::channel();
            let mut writer = ProgressWriter::new(Gate(gate), timeout).unwrap();
            let error = if flush {
                writer.flush().unwrap_err()
            } else {
                writer.write(b"x").unwrap_err()
            };
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            // A later write must fail immediately instead of accepting a late ack.
            assert!(writer.write(b"late response").is_err());
            drop(release);
        }
    }
    #[test]
    fn successful_io_preserves_bytes_and_eof() {
        let timeout = Duration::from_secs(1);
        let content = vec![7; CHUNK * 3 + 1];
        let mut reader = ProgressReader::new(Cursor::new(content.clone()), timeout).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, content);
        let mut writer = ProgressWriter::new(io::sink(), timeout).unwrap();
        writer.write_all(&content).unwrap();
        writer.flush().unwrap();
    }

    struct InterruptOnce<T>(bool, T);
    impl<T: Read> Read for InterruptOnce<T> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.0) {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                self.1.read(bytes)
            }
        }
    }
    impl<T: Write> Write for InterruptOnce<T> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.0) {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                self.1.write(bytes)
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            self.1.flush()
        }
    }
    #[test]
    fn interrupted_os_calls_remain_retryable() {
        let timeout = Duration::from_secs(1);
        let mut reader =
            ProgressReader::new(InterruptOnce(true, Cursor::new(b"bytes")), timeout).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"bytes");
        let mut writer = ProgressWriter::new(InterruptOnce(true, io::sink()), timeout).unwrap();
        writer.write_all(&bytes).unwrap();
        writer.flush().unwrap();
    }
}
