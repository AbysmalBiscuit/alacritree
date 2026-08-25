//! Supplies the level-triggered console readiness that the PTY reader accepts
//! but never sends.
//!
//! Windows has no readiness to report for a console pipe, so the reader
//! emulates it: a completion packet reaches the poller through the waker
//! `piper` holds for its pipe.  `piper` installs that waker only when a drain
//! comes up empty, which is the ordinary contract — poll until pending.  The
//! reader takes `PollMode::Level` and then never posts a packet of its own
//! after a drain returns data, so a drain that leaves bytes behind ends with
//! the pipe holding data and nothing able to announce it.  The loop sleeps
//! until something unrelated arrives — a keystroke, a resize, the child
//! exiting — which is why a pane streaming megabytes only advances while the
//! user types.
//!
//! Wrapping the reader posts that packet.  What has to hold is that the
//! packets a visit posts never outnumber the visits they buy, or the backlog
//! doubles every round until a wait returns nothing but stale packets.  A visit
//! that emptied the pipe announces nothing at all, because reaching that empty
//! read is what makes `piper` install its waker, and the waker announces the
//! next byte.  Every other visit announces once — see `HANDBACK`.
//!
//! One case still posts more than it consumes: a visit that cannot reach the
//! terminal lock reads again without parsing, and each of those reads
//! announces.  Nothing inside a reader can see a visit boundary, so this is
//! bounded only by how long the UI thread holds the lock.  It is worth knowing
//! about; it is not what stalls a pane.

use std::io::{self, Read};
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{
    ChildEvent, EventedPty, EventedReadWrite, PTY_READ_WRITE_TOKEN, Pty,
};
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, PollMode, Poller};

/// Mirrors `alacritty_terminal::event_loop::MAX_LOCKED_READ`, which is private.
const MAX_LOCKED_READ: usize = u16::MAX as usize;

/// How much one read may hand back.
///
/// The read loop reserves the terminal for as long as it runs and stops once
/// it has parsed `MAX_LOCKED_READ`, but it checks that cap only after parsing
/// whatever the last read returned.  A read carrying the whole cap therefore
/// ends the visit by itself, which is what holds an uncontended visit to one
/// announcement.  `take` fills the whole cap even across a refill for the same
/// reason: a short hand-back leaves the visit under the cap, so it reads once
/// more and announces once more.
///
/// Handing back less does not bound the parse — the cap already does that.  It
/// costs the visit a second read, and two packets per visit buy two visits,
/// which post four, and the backlog doubles until every wait returns a thousand
/// stale packets.  The loop drains the channel carrying keystrokes once per
/// wait, and the writable packet that finally carries a Ctrl-C to the child
/// goes on the tail of that queue, so it waits behind the whole backlog.
const HANDBACK: usize = MAX_LOCKED_READ;

/// How much one refill may pull out of the console pipe ahead of the read loop.
///
/// The pipe between the console and the read loop is a ring, filled by a thread
/// that parks when it runs out of room.  Staging ahead of the parse lets that
/// thread keep running at the console's pace while the loop works through what
/// it already holds.  It does not take much: `piper` wakes the filler after
/// every chunk it copies out, so the ring does not have to be emptied to unpark
/// it, and one visit in hand is enough to keep the loop from waiting on a
/// refill.
///
/// What is staged past that is output already committed to the screen, so it is
/// what a Ctrl-C waits through before the prompt comes back.  The 1 MiB ring
/// underneath is in that path too and dominates it; this is the half this side
/// owns.  The value is reasoned, not measured — compare TermMarkV2 before
/// changing it.
const DRAIN_AHEAD: usize = 2 * HANDBACK;

/// Bytes taken out of the console pipe ahead of the read loop, and how far the
/// loop has got through them.
#[derive(Default)]
struct Staging {
    buf: Vec<u8>,
    taken: usize,
    /// Whether the last refill ended on a read that returned nothing, which is
    /// what `UnblockedReader` reports once `piper` has taken the waker.  A
    /// refill that stopped at `DRAIN_AHEAD` instead never asked for a byte
    /// `piper` could not supply, so no waker was installed and nothing but this
    /// side can announce what is left in the ring.
    ///
    /// The converse does not quite hold: `piper` yields on a non-empty ring
    /// about once in a hundred drains, and that also reads as nothing.  It
    /// wakes the reader on its way out, so the announcement comes through the
    /// waker rather than from here.
    rearmed: bool,
}

impl Staging {
    /// Empty `source` into the buffer, up to `DRAIN_AHEAD`.
    ///
    /// How far this got decides who announces next, so it records whether it
    /// reached the read that returned nothing.
    fn refill(&mut self, source: &mut impl Read) -> io::Result<()> {
        self.buf.clear();
        self.taken = 0;
        self.rearmed = false;

        while self.buf.len() < DRAIN_AHEAD {
            let base = self.buf.len();
            self.buf.resize(base + HANDBACK, 0);
            match source.read(&mut self.buf[base..]) {
                Ok(0) => {
                    self.buf.truncate(base);
                    self.rearmed = true;
                    break;
                },
                Ok(read) => self.buf.truncate(base + read),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => self.buf.truncate(base),
                // `UnblockedReader` never reports this, and a source that did
                // would not have armed a waker on the way out, so leave the
                // announcement to this side.
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    self.buf.truncate(base);
                    break;
                },
                Err(err) => {
                    self.buf.truncate(base);
                    return Err(err);
                },
            }
        }
        Ok(())
    }

    /// Hand the read loop a whole visit's worth, refilling as the buffer runs
    /// out, and stopping short only when the pipe has no more to give.
    ///
    /// Returns how much was handed back and whether anything is left for the
    /// caller to announce, either staged here or behind a ring nothing else
    /// will speak for.
    fn take(&mut self, out: &mut [u8], source: &mut impl Read) -> io::Result<(usize, bool)> {
        let want = out.len().min(HANDBACK);
        let mut read = 0;

        while read < want {
            if self.taken == self.buf.len() {
                self.refill(source)?;
                if self.buf.is_empty() {
                    break;
                }
            }

            let end = (self.taken + want - read).min(self.buf.len());
            let staged = &self.buf[self.taken..end];
            out[read..read + staged.len()].copy_from_slice(staged);
            read += staged.len();
            self.taken = end;
        }

        Ok((read, self.taken < self.buf.len() || !self.rearmed))
    }
}

/// Owns the PTY because `EventedReadWrite` hands out `&mut Self::Reader`, and
/// only the type holding the reader can supply that.
pub struct RearmingReader {
    pty: Pty,
    poller: Option<Arc<Poller>>,
    staged: Staging,
}

impl Read for RearmingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Self { pty, staged, poller } = self;
        let (read, staged_behind) = staged.take(buf, pty.reader())?;

        if staged_behind && let Some(poller) = poller {
            let _ = poller.post(CompletionPacket::new(Event::readable(PTY_READ_WRITE_TOKEN)));
        }
        Ok(read)
    }
}

pub struct RearmingPty {
    reader: RearmingReader,
}

impl RearmingPty {
    pub fn new(pty: Pty) -> Self {
        Self { reader: RearmingReader { pty, poller: None, staged: Staging::default() } }
    }
}

impl EventedReadWrite for RearmingPty {
    type Reader = RearmingReader;
    type Writer = <Pty as EventedReadWrite>::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        poll_opts: PollMode,
    ) -> io::Result<()> {
        self.reader.poller = Some(poll.clone());
        unsafe { self.reader.pty.register(poll, interest, poll_opts) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        poll_opts: PollMode,
    ) -> io::Result<()> {
        self.reader.poller = Some(poll.clone());
        self.reader.pty.reregister(poll, interest, poll_opts)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.reader.poller = None;
        self.reader.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.reader.pty.writer()
    }
}

impl EventedPty for RearmingPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.reader.pty.next_child_event()
    }
}

impl OnResize for RearmingPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.reader.pty.on_resize(window_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the read loop hands a read: its parse buffer, always room for more
    /// than one hand-back.
    fn parse_buffer() -> Vec<u8> {
        vec![0; 4 * HANDBACK]
    }

    /// A source that gives back at most `chunk` bytes at a time, the way
    /// `piper` does when the reader is at the filling thread's heels.
    struct ShortReads {
        remaining: usize,
        chunk: usize,
    }

    impl Read for ShortReads {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = buf.len().min(self.chunk).min(self.remaining);
            buf[..read].fill(b'x');
            self.remaining -= read;
            Ok(read)
        }
    }

    /// A refill built out of short reads does not land on a multiple of the
    /// visit cap, so somewhere in it a hand-back runs out mid-visit.  Stopping
    /// there leaves the visit under the cap, so it reads again and announces
    /// again: one visit, two packets, and the backlog grows every refill.
    /// Crossing into the next refill instead keeps the visit to one read.
    #[test]
    fn a_short_read_still_fills_a_whole_handback() {
        let mut source = ShortReads { remaining: 64 * HANDBACK, chunk: 20_000 };
        let mut staging = Staging::default();
        let mut buf = parse_buffer();

        for take in 1..=8 {
            let (read, _) = staging.take(&mut buf, &mut source).unwrap();
            assert_eq!(read, HANDBACK, "take {take} left the visit short of its cap");
        }
    }

    /// The read loop parses what a read returned before re-checking its own
    /// `MAX_LOCKED_READ` cap, so a take carrying the cap ends the visit by
    /// itself.  A shorter one costs the visit a second take, and every take
    /// that leaves bytes behind announces the pipe again — the packets one
    /// visit posts are the visits the next wait runs, so the backlog doubles.
    #[test]
    fn one_take_serves_a_whole_visit() {
        let mut source = io::Cursor::new(vec![b'x'; 4 * MAX_LOCKED_READ]);
        let mut staging = Staging::default();

        let (read, staged_behind) = staging.take(&mut parse_buffer(), &mut source).unwrap();

        assert!(read >= MAX_LOCKED_READ, "a visit stops after {MAX_LOCKED_READ}, got {read}");
        assert!(staged_behind, "the rest of the drain still has to be announced");
    }

    /// Emptying the pipe leaves `piper` holding the waker, and that is what
    /// announces the next byte.  Announcing here as well queues a packet the
    /// loop wakes for and finds nothing behind.
    #[test]
    fn a_drained_pipe_announces_nothing() {
        let mut source = io::Cursor::new(b"hello".to_vec());
        let mut staging = Staging::default();

        assert_eq!(staging.take(&mut parse_buffer(), &mut source).unwrap(), (5, false));
        assert_eq!(staging.take(&mut parse_buffer(), &mut source).unwrap(), (0, false));
    }

    /// A refill that stopped at `DRAIN_AHEAD` never asked `piper` for a byte it
    /// could not supply, so `piper` holds no waker and the ring may still be
    /// full.  Handing back the last staged byte has to announce anyway: being
    /// wrong costs one wakeup that finds nothing, and staying quiet costs a
    /// pane that stops until the user types.
    #[test]
    fn a_refill_that_hit_the_cap_announces_its_last_byte() {
        let mut source = io::Cursor::new(vec![b'x'; 4 * DRAIN_AHEAD]);
        let mut staging = Staging::default();
        let mut buf = parse_buffer();

        let mut taken = 0;
        while taken < DRAIN_AHEAD {
            let (read, staged_behind) = staging.take(&mut buf, &mut source).unwrap();
            taken += read;
            assert!(staged_behind, "the ring was never drained, so only this side can announce");
        }
    }

    /// Every visit takes its cap and announces the remainder once, until the
    /// drain runs out.  Anything above one packet per visit compounds.
    #[test]
    fn a_drain_announces_once_per_visit() {
        let staged = 4 * MAX_LOCKED_READ + 100;
        let mut source = io::Cursor::new(vec![b'x'; staged]);
        let mut staging = Staging::default();
        let mut buf = parse_buffer();

        let mut announced = 0;
        let mut visits = 0;
        loop {
            let (read, staged_behind) = staging.take(&mut buf, &mut source).unwrap();
            if read == 0 {
                break;
            }
            visits += 1;
            announced += usize::from(staged_behind);
        }

        assert_eq!(visits, staged.div_ceil(MAX_LOCKED_READ));
        assert_eq!(announced, visits - 1, "only the last visit finds nothing staged behind it");
    }
}
