use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossterm::event::{self, Event};
use hz_core::{HzError, HzResult};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};

const EVENT_READER_POLL: Duration = Duration::from_millis(2);
const EVENT_READER_CHANNEL_CAPACITY: usize = 1024;
const READY_EVENT_DRAIN_LIMIT: usize = 1024;
type EventReaderParts = (Receiver<io::Result<Event>>, Arc<AtomicBool>, JoinHandle<()>);

pub(crate) struct TerminalEventReader {
    thread_name: &'static str,
    rx: Receiver<io::Result<Event>>,
    shutdown: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

impl TerminalEventReader {
    pub(crate) fn start(thread_name: &'static str) -> HzResult<Self> {
        let (rx, shutdown, handle) = spawn_reader_thread(thread_name)?;

        Ok(Self {
            thread_name,
            rx,
            shutdown: Some(shutdown),
            handle: Some(handle),
        })
    }

    pub(crate) async fn read_timeout(&mut self, timeout: Duration) -> HzResult<Option<Event>> {
        if timeout.is_zero() {
            return self.try_read();
        }

        match tokio::time::timeout(timeout, self.rx.recv()).await {
            Ok(Some(result)) => result.map(Some).map_err(HzError::Io),
            Ok(None) => Err(reader_stopped_error()),
            Err(_) => Ok(None),
        }
    }

    pub(crate) fn try_read(&mut self) -> HzResult<Option<Event>> {
        match self.rx.try_recv() {
            Ok(result) => result.map(Some).map_err(HzError::Io),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(reader_stopped_error()),
        }
    }

    pub(crate) fn pause(&mut self) -> PausedTerminalEventReader<'_> {
        self.stop();
        self.clear_pending_events();
        PausedTerminalEventReader {
            reader: self,
            resumed: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_receiver(rx: Receiver<io::Result<Event>>) -> Self {
        Self {
            thread_name: "hz-test-events",
            rx,
            shutdown: None,
            handle: None,
        }
    }

    fn restart(&mut self) -> HzResult<()> {
        let (rx, shutdown, handle) = spawn_reader_thread(self.thread_name)?;
        self.rx = rx;
        self.shutdown = Some(shutdown);
        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.store(true, Ordering::Relaxed);
        }
        if self.handle.is_some() {
            // Unblock a reader thread parked on a full bounded channel.
            self.rx.close();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn clear_pending_events(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

pub(crate) struct PausedTerminalEventReader<'a> {
    reader: &'a mut TerminalEventReader,
    resumed: bool,
}

impl PausedTerminalEventReader<'_> {
    pub(crate) fn resume(mut self) -> HzResult<()> {
        self.reader.restart()?;
        self.resumed = true;
        Ok(())
    }
}

impl Drop for PausedTerminalEventReader<'_> {
    fn drop(&mut self) {
        if !self.resumed {
            let _ = self.reader.restart();
        }
    }
}

impl Drop for TerminalEventReader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_reader_thread(thread_name: &'static str) -> io::Result<EventReaderParts> {
    let (tx, rx) = mpsc::channel(EVENT_READER_CHANNEL_CAPACITY);
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = Arc::clone(&shutdown);
    let handle = thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || read_terminal_events(tx, thread_shutdown))?;

    Ok((rx, shutdown, handle))
}

fn read_terminal_events(tx: Sender<io::Result<Event>>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        match event::poll(EVENT_READER_POLL) {
            Ok(true) if shutdown.load(Ordering::Relaxed) => break,
            Ok(true) => match event::read() {
                Ok(event) => {
                    if !send_terminal_event(&tx, Ok(event)) {
                        break;
                    }
                }
                Err(error) => {
                    let _ = send_terminal_event(&tx, Err(error));
                    break;
                }
            },
            Ok(false) => {
                if tx.is_closed() {
                    break;
                }
            }
            Err(error) => {
                let _ = send_terminal_event(&tx, Err(error));
                break;
            }
        }
    }
    drain_ready_terminal_events();
}

fn drain_ready_terminal_events() {
    drain_ready_events(|| event::poll(Duration::ZERO), || event::read().map(|_| ()));
}

fn send_terminal_event(tx: &Sender<io::Result<Event>>, event: io::Result<Event>) -> bool {
    tx.blocking_send(event).is_ok()
}

fn drain_ready_events(
    mut poll_ready: impl FnMut() -> io::Result<bool>,
    mut read_event: impl FnMut() -> io::Result<()>,
) {
    for _ in 0..READY_EVENT_DRAIN_LIMIT {
        match poll_ready() {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }

        if read_event().is_err() {
            break;
        }
    }
}

fn reader_stopped_error() -> HzError {
    HzError::Io(io::Error::other("terminal event reader stopped"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_read_returns_queued_event() {
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(Ok(Event::Resize(80, 24))).unwrap();
        let mut reader = TerminalEventReader::from_receiver(rx);

        assert_eq!(reader.try_read().unwrap(), Some(Event::Resize(80, 24)));
        assert_eq!(reader.try_read().unwrap(), None);
    }

    #[test]
    fn try_read_returns_queued_error() {
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(Err(io::Error::other("event failed"))).unwrap();
        let mut reader = TerminalEventReader::from_receiver(rx);

        assert_eq!(reader.try_read().unwrap_err().to_string(), "event failed");
    }

    #[test]
    fn pause_discards_events_already_queued_for_hz() {
        let (tx, rx) = mpsc::channel(2);
        tx.try_send(Ok(Event::Resize(80, 24))).unwrap();
        tx.try_send(Ok(Event::Resize(100, 30))).unwrap();
        let mut reader = TerminalEventReader::from_receiver(rx);

        let mut paused = reader.pause();
        paused.resumed = true;
        drop(paused);

        assert_eq!(reader.try_read().unwrap(), None);
    }

    #[test]
    fn ready_event_drain_stops_at_limit_when_input_stays_ready() {
        let mut reads = 0;

        drain_ready_events(
            || Ok(true),
            || {
                reads += 1;
                Ok(())
            },
        );

        assert_eq!(reads, READY_EVENT_DRAIN_LIMIT);
    }

    #[test]
    fn terminal_event_send_unblocks_when_receiver_closes() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Ok(Event::Resize(80, 24))).unwrap();
        let handle = thread::spawn(move || send_terminal_event(&tx, Ok(Event::Resize(100, 30))));

        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());

        rx.close();
        assert!(!handle.join().unwrap());
    }
}
